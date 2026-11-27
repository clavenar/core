"""Launch Clavenar in QEMU with a journaled mock planner on the host."""

import argparse
from collections.abc import Callable
from contextlib import closing
from pathlib import Path
import socket
import sqlite3
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory
from typing import BinaryIO
from uuid import uuid4

KERNEL = Path("target/riscv64gc-unknown-none-elf/release/clavenar-kernel")
JOURNAL = Path("target/host-bridge.sqlite3")
SOCKET_TIMEOUT = 5

PLAN = struct.Struct("<BBBQB")
VERSION = 1
PLAN_REQUEST = 1
PLAN_REPLY = 2
FIND_GREETING = 1
BUDGET_EXHAUSTED = 0
SEARCH_GREETING = 1

MOCK_MAX_UNITS = 2
MOCK_USED_UNITS = 1
MOCK_ACTIONS = {"search": SEARCH_GREETING, "invalid-plan": 2, "silent-plan": None}
PROBE_REPLIES = {"probe-only": b"PONG", "invalid-probe": b"PONX", "silent-probe": b""}


class Journal:
    def __init__(self, db: sqlite3.Connection):
        self.db = db
        (mode,) = db.execute("PRAGMA journal_mode = WAL").fetchone()
        if mode != "wal":
            raise RuntimeError("could not enable WAL for the host journal")
        db.executescript("""
            PRAGMA synchronous = FULL;
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY NOT NULL,
                budget_units INTEGER NOT NULL CHECK (budget_units >= 0)
            );
            CREATE TABLE IF NOT EXISTS requests (
                session_id TEXT NOT NULL REFERENCES sessions(id),
                request_id TEXT NOT NULL,
                state TEXT NOT NULL,
                max_units INTEGER NOT NULL CHECK (max_units > 0),
                used_units INTEGER NOT NULL DEFAULT 0
                    CHECK (used_units BETWEEN 0 AND max_units),
                reply INTEGER CHECK (reply BETWEEN 0 AND 255),
                PRIMARY KEY (session_id, request_id),
                CHECK (
                    (state = 'in_flight' AND used_units = 0 AND reply IS NULL)
                    OR (state = 'denied' AND used_units = 0 AND reply IS 0)
                    OR (state = 'settled' AND reply IS NOT NULL AND reply > 0)
                )
            );
        """)

    def create_session(self, budget_units: int) -> str:
        session = uuid4().hex
        with self.db:
            self.db.execute(
                "INSERT INTO sessions (id, budget_units) VALUES (?, ?)",
                (session, budget_units),
            )
        return session

    def handle_plan(
        self,
        session: str,
        request_id: int,
        run_mock: Callable[[], tuple[int, int] | None],
    ) -> int | None:
        # Store unsigned wire IDs as fixed-width hex for SQLite and host inspection.
        key = (session, f"{request_id:016x}")
        with self.db:
            self.db.execute("BEGIN IMMEDIATE")
            stored = self.db.execute(
                "SELECT reply FROM requests WHERE session_id = ? AND request_id = ?",
                key,
            ).fetchone()
            if stored is not None:
                return stored[0]

            remaining = self.db.execute(
                """
                SELECT budget_units - COALESCE(SUM(
                    CASE WHEN state = 'in_flight' THEN max_units ELSE used_units END
                ), 0)
                FROM sessions LEFT JOIN requests ON id = session_id
                WHERE id = ?
                GROUP BY id
                """,
                (session,),
            ).fetchone()
            if remaining is None:
                raise ValueError("unknown VM session")
            can_dispatch = remaining[0] >= MOCK_MAX_UNITS
            state, reply = ("in_flight", None) if can_dispatch else ("denied", BUDGET_EXHAUSTED)
            self.db.execute(
                """
                INSERT INTO requests (session_id, request_id, state, max_units, reply)
                VALUES (?, ?, ?, ?, ?)
                """,
                (*key, state, MOCK_MAX_UNITS, reply),
            )

        if not can_dispatch:
            return BUDGET_EXHAUSTED

        # The reservation is committed before dispatch. An unknown outcome keeps it.
        result = run_mock()
        if result is None:
            return None

        action, used = result
        if not (1 <= action <= 255 and 0 <= used <= MOCK_MAX_UNITS):
            raise ValueError("mock result falls outside its reservation or reply format")
        with self.db:
            updated = self.db.execute(
                """
                UPDATE requests SET state = 'settled', used_units = ?, reply = ?
                WHERE session_id = ? AND request_id = ? AND state = 'in_flight'
                """,
                (used, action, *key),
            ).rowcount
            if updated != 1:
                raise RuntimeError("could not settle the host plan reservation")
        return action


def serve_plans(
    peer: socket.socket,
    wire: BinaryIO,
    journal: Journal,
    session: str,
    action: int | None,
) -> None:
    while first := wire.read(1):
        if first[0] != PLAN.size:
            raise ValueError("invalid plan request length")
        peer.settimeout(SOCKET_TIMEOUT)
        body = wire.read(PLAN.size - 1)
        if len(body) != PLAN.size - 1:
            raise ValueError("truncated plan request")
        _, version, kind, request_id, selector = PLAN.unpack(first + body)
        if (version, kind, selector) != (VERSION, PLAN_REQUEST, FIND_GREETING) or request_id == 0:
            raise ValueError("invalid plan request")

        reply = journal.handle_plan(
            session,
            request_id,
            lambda: None if action is None else (action, MOCK_USED_UNITS),
        )
        if reply is not None:
            peer.sendall(PLAN.pack(PLAN.size, VERSION, PLAN_REPLY, request_id, reply))
        peer.settimeout(None)


def run_qemu(
    boot_reply: bytes,
    serve: Callable[[socket.socket, BinaryIO], None] | None = None,
) -> int:
    with TemporaryDirectory(prefix="clavenar-bridge-") as directory:
        path = str(Path(directory) / "bridge.sock")
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
            listener.settimeout(SOCKET_TIMEOUT)
            listener.bind(path)
            listener.listen(1)
            guest = subprocess.Popen([
                "qemu-system-riscv64", "-machine", "virt", "-accel", "tcg",
                "-m", "128M", "-smp", "1", "-bios", "default", "-nographic",
                "-nic", "none", "-kernel", str(KERNEL),
                "-global", "virtio-mmio.force-legacy=off",
                "-chardev", f"socket,id=bridge,path={path},server=off",
                "-device", "virtio-serial-device,max_ports=1",
                "-device", "virtconsole,chardev=bridge,nr=0",
            ])
            try:
                peer, _ = listener.accept()
                with peer, peer.makefile("rb") as wire:
                    peer.settimeout(SOCKET_TIMEOUT)
                    if wire.read(4) != b"PING":
                        raise ValueError("expected PING from the guest")
                    if boot_reply:
                        peer.sendall(boot_reply)
                    if serve is not None:
                        peer.settimeout(None)
                        serve(peer, wire)
                    return guest.wait()
            finally:
                if guest.poll() is None:
                    guest.terminate()
                    guest.wait()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--journal", type=Path, default=JOURNAL, help="SQLite journal path")
    parser.add_argument("--budget-units", type=int, default=2, help="mock units per VM session")
    parser.add_argument(
        "--mode",
        choices=(*MOCK_ACTIONS, *PROBE_REPLIES),
        default="search",
        help="host response to exercise",
    )
    args = parser.parse_args()
    if not 0 <= args.budget_units < 2**63:
        parser.error("--budget-units must be a nonnegative SQLite integer")
    if not KERNEL.is_file():
        parser.error(f"build the RV64 release ELF from the Core repository first ({KERNEL})")

    if args.mode in PROBE_REPLIES:
        return run_qemu(PROBE_REPLIES[args.mode])

    path = args.journal.resolve()
    with closing(sqlite3.connect(path, isolation_level=None)) as db:
        journal = Journal(db)
        session = journal.create_session(args.budget_units)
        print(
            f"host session {session} budget_units={args.budget_units} journal={path}",
            file=sys.stderr,
            flush=True,
        )
        action = MOCK_ACTIONS[args.mode]
        return run_qemu(b"PONG", lambda peer, wire: serve_plans(peer, wire, journal, session, action))


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, RuntimeError, sqlite3.Error) as error:
        print(f"host bridge: {error}", file=sys.stderr)
        raise SystemExit(1) from None
    except KeyboardInterrupt:
        raise SystemExit(130) from None

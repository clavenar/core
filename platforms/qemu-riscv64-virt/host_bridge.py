"""Launch Clavenar in QEMU with journaled planning and read-only host tools."""

import argparse
from collections.abc import Callable
from contextlib import closing
from functools import partial
from pathlib import Path
import socket
import sqlite3
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory
from typing import BinaryIO, NamedTuple
from uuid import uuid4

from mcp_backend import search_fixture
from ollama_backend import MAX_TOKENS, TokenUsage, choose_action

KERNEL = Path("target/riscv64gc-unknown-none-elf/release/clavenar-kernel")
JOURNAL = Path("target/host-bridge.sqlite3")
SOCKET_TIMEOUT = 5

PLAN = struct.Struct("<BBBQB")
TOOL = struct.Struct("<BBBQBQ")
COMBINED = struct.Struct("<BBBQBQQ")
VERSION = 1
PLAN_REQUEST = 1
PLAN_REPLY = 2
TOOL_REQUEST = 3
TOOL_REPLY = 4
MOCK_GREETING = 1
MODEL_GREETING = 2
MCP_SEARCH = 3
COMBINED_PLAN = 4
COMBINED_TOOL = 5
BUDGET_EXHAUSTED = 0
SEARCH_GREETING = 1
TOOL_OK = 1
INVALID_REPLY = 2
SKIP = 3
SERVICE_UNAVAILABLE = 4

MOCK_MAX_UNITS = 2
MOCK_USED_UNITS = 1
MOCK_ACTIONS = {"search": SEARCH_GREETING, "invalid-plan": INVALID_REPLY, "silent-plan": None}
PROBE_REPLIES = {"probe-only": b"PONG", "invalid-probe": b"PONX", "silent-probe": b""}


class Reply(NamedTuple):
    status: int
    data: bytes = b""


class RequestResult(NamedTuple):
    reply: Reply
    usage: int | TokenUsage


RequestRunner = Callable[[str], RequestResult | None]


class HostRequest(NamedTuple):
    request_id: int
    selector: int
    goal_id: int | None = None
    launch_id: int | None = None

    @property
    def is_tool(self) -> bool:
        return self.selector in (MCP_SEARCH, COMBINED_TOOL)


class Service(NamedTuple):
    run: RequestRunner
    max_units: int


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
            CREATE TABLE IF NOT EXISTS model_sessions (
                session_id TEXT PRIMARY KEY NOT NULL REFERENCES sessions(id),
                model TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS token_usage (
                session_id TEXT NOT NULL REFERENCES model_sessions(session_id),
                request_id TEXT NOT NULL,
                prompt_tokens INTEGER NOT NULL CHECK (prompt_tokens >= 0),
                generation_tokens INTEGER NOT NULL CHECK (generation_tokens >= 0),
                PRIMARY KEY (session_id, request_id),
                FOREIGN KEY (session_id, request_id) REFERENCES requests(session_id, request_id)
            );
            CREATE TABLE IF NOT EXISTS mcp_calls (
                session_id TEXT NOT NULL,
                request_id TEXT NOT NULL,
                goal_id TEXT NOT NULL,
                result BLOB CHECK (length(result) <= 20),
                PRIMARY KEY (session_id, request_id),
                FOREIGN KEY (session_id, request_id) REFERENCES requests(session_id, request_id)
            );
            CREATE TABLE IF NOT EXISTS mcp_budgets (
                session_id TEXT PRIMARY KEY NOT NULL REFERENCES sessions(id),
                budget_calls INTEGER NOT NULL CHECK (budget_calls >= 0)
            );
            CREATE TABLE IF NOT EXISTS request_owners (
                session_id TEXT NOT NULL,
                request_id TEXT NOT NULL,
                goal_id TEXT NOT NULL,
                launch_id TEXT NOT NULL,
                PRIMARY KEY (session_id, request_id),
                FOREIGN KEY (session_id, request_id) REFERENCES requests(session_id, request_id)
            );
        """)

    def create_session(
        self, budget_units: int, model: str | None = None, budget_calls: int | None = None
    ) -> str:
        session = uuid4().hex
        with self.db:
            self.db.execute("BEGIN IMMEDIATE")
            self.db.execute(
                "INSERT INTO sessions (id, budget_units) VALUES (?, ?)",
                (session, budget_units),
            )
            if model is not None:
                self.db.execute(
                    "INSERT INTO model_sessions (session_id, model) VALUES (?, ?)",
                    (session, model),
                )
            if budget_calls is not None:
                self.db.execute(
                    "INSERT INTO mcp_budgets (session_id, budget_calls) VALUES (?, ?)",
                    (session, budget_calls),
                )
        return session

    def handle_request(
        self,
        session: str,
        request: HostRequest,
        service: Service,
    ) -> Reply | None:
        # Store unsigned wire IDs as fixed-width hex for SQLite and host inspection.
        key = (session, f"{request.request_id:016x}")
        goal = None if request.goal_id is None else f"{request.goal_id:016x}"
        launch = None if request.launch_id is None else f"{request.launch_id:016x}"
        tool_goal = goal if request.is_tool else None
        owner_goal = goal if launch is not None else None
        max_units = service.max_units
        with self.db:
            self.db.execute("BEGIN IMMEDIATE")
            stored = self.db.execute(
                """
                SELECT reply, result, mcp_calls.goal_id, request_owners.goal_id, launch_id FROM requests
                LEFT JOIN mcp_calls USING (session_id, request_id)
                LEFT JOIN request_owners USING (session_id, request_id)
                WHERE requests.session_id = ? AND requests.request_id = ?
                """,
                key,
            ).fetchone()
            if stored is not None:
                status, data, *stored_owner = stored
                if stored_owner != [tool_goal, owner_goal, launch]:
                    raise ValueError("repeated host request changed its owner or service")
                return None if status is None else Reply(status, data or b"")

            remaining = self.db.execute(
                """
                SELECT (CASE WHEN :tool THEN COALESCE(budget_calls, budget_units)
                             ELSE budget_units END) - (
                    SELECT COALESCE(SUM(
                        CASE WHEN state = 'in_flight' THEN max_units ELSE used_units END
                    ), 0)
                    FROM requests LEFT JOIN mcp_calls USING (session_id, request_id)
                    WHERE requests.session_id = :session
                      AND (mcp_calls.request_id IS NOT NULL) = :tool
                )
                FROM sessions LEFT JOIN mcp_budgets ON id = session_id
                WHERE id = :session
                """,
                {"session": session, "tool": request.is_tool},
            ).fetchone()
            if remaining is None:
                raise ValueError("unknown VM session")
            can_dispatch = remaining[0] >= max_units
            state, status = ("in_flight", None) if can_dispatch else ("denied", BUDGET_EXHAUSTED)
            self.db.execute(
                """
                INSERT INTO requests (session_id, request_id, state, max_units, reply)
                VALUES (?, ?, ?, ?, ?)
                """,
                (*key, state, max_units, status),
            )
            if tool_goal is not None:
                self.db.execute(
                    "INSERT INTO mcp_calls (session_id, request_id, goal_id) VALUES (?, ?, ?)",
                    (*key, tool_goal),
                )
            if launch is not None:
                self.db.execute(
                    "INSERT INTO request_owners (session_id, request_id, goal_id, launch_id) VALUES (?, ?, ?, ?)",
                    (*key, owner_goal, launch),
                )

        if not can_dispatch:
            return Reply(BUDGET_EXHAUSTED)

        # The reservation is committed before dispatch. An unknown outcome keeps it.
        result = service.run(f"{session}/{key[1]}")
        if result is None:
            return None

        reply, usage = result
        used = sum(usage) if isinstance(usage, TokenUsage) else usage
        if not (1 <= reply.status <= 255 and 0 <= used <= max_units):
            raise ValueError("host result falls outside its reservation or reply format")
        with self.db:
            self.db.execute("BEGIN IMMEDIATE")
            updated = self.db.execute(
                """
                UPDATE requests SET state = 'settled', used_units = ?, reply = ?
                WHERE session_id = ? AND request_id = ? AND state = 'in_flight'
                """,
                (used, reply.status, *key),
            ).rowcount
            if updated != 1:
                raise RuntimeError("could not settle the host request reservation")
            if isinstance(usage, TokenUsage):
                self.db.execute(
                    """
                    INSERT INTO token_usage
                        (session_id, request_id, prompt_tokens, generation_tokens)
                    VALUES (?, ?, ?, ?)
                    """,
                    (*key, *usage),
                )
            if request.is_tool and reply.status == TOOL_OK:
                self.db.execute(
                    "UPDATE mcp_calls SET result = ? WHERE session_id = ? AND request_id = ?",
                    (reply.data, *key),
                )
        return reply


def run_ollama_plan(
    model: str, port: int, _operation_id: str, *, combined: bool = False
) -> RequestResult | None:
    result = choose_action(model, port, combined=combined)
    if result is None:
        return None
    decision, usage = result
    actions = {"search": SEARCH_GREETING, "skip": SKIP, "unavailable": SERVICE_UNAVAILABLE}
    return RequestResult(Reply(actions.get(decision, INVALID_REPLY)), usage)


def run_mcp_search(port: int, operation_id: str) -> RequestResult | None:
    result = search_fixture(port, operation_id)
    if result is None:
        return None
    if result.used_calls == 0:
        reply = Reply(SERVICE_UNAVAILABLE)
    elif result.text is None:
        reply = Reply(INVALID_REPLY)
    else:
        reply = Reply(TOOL_OK, result.text)
    return RequestResult(reply, result.used_calls)


def decode_request(frame: bytes) -> HostRequest:
    length, version, kind, request_id, selector = PLAN.unpack_from(frame)
    if version != VERSION or request_id == 0:
        raise ValueError("invalid host request header")
    if kind == PLAN_REQUEST and length == PLAN.size and selector in (MOCK_GREETING, MODEL_GREETING):
        return HostRequest(request_id, selector)
    if kind == TOOL_REQUEST and length == TOOL.size and selector == MCP_SEARCH:
        goal = TOOL.unpack(frame)[5]
        if goal == 0:
            raise ValueError("invalid host tool goal ID")
        return HostRequest(request_id, selector, goal)
    if length == COMBINED.size and (kind, selector) in (
        (PLAN_REQUEST, COMBINED_PLAN), (TOOL_REQUEST, COMBINED_TOOL)
    ):
        goal, launch = COMBINED.unpack(frame)[5:]
        if not 0 < goal < launch < request_id:
            raise ValueError("invalid combined request owner")
        return HostRequest(request_id, selector, goal, launch)
    raise ValueError("invalid host request kind or selector")


def serve_requests(
    peer: socket.socket,
    wire: BinaryIO,
    journal: Journal,
    session: str,
    services: dict[int, Service],
) -> None:
    while first := wire.read(1):
        length = first[0]
        if length not in (PLAN.size, TOOL.size, COMBINED.size):
            raise ValueError("invalid host request length")
        peer.settimeout(SOCKET_TIMEOUT)
        body = wire.read(length - 1)
        if len(body) != length - 1:
            raise ValueError("truncated host request")
        request = decode_request(first + body)
        service = services.get(request.selector)
        reply = Reply(SERVICE_UNAVAILABLE)
        if service is not None:
            reply = journal.handle_request(session, request, service)
            if reply is None and request.selector != MOCK_GREETING:
                reply = Reply(SERVICE_UNAVAILABLE)
        if reply is not None:
            length = PLAN.size + len(reply.data)
            reply_kind = TOOL_REPLY if request.is_tool else PLAN_REPLY
            header = PLAN.pack(length, VERSION, reply_kind, request.request_id, reply.status)
            peer.sendall(header + reply.data)
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
        "--budget-tokens", type=int, default=MAX_TOKENS, help="tokens per model session"
    )
    parser.add_argument("--ollama-model", help="name of an installed local Ollama model")
    parser.add_argument("--ollama-port", type=int, default=11434, help="Ollama port on 127.0.0.1")
    parser.add_argument("--mcp-port", type=int, help="MCP server port on 127.0.0.1")
    parser.add_argument("--budget-calls", type=int, default=1, help="MCP tool calls per VM session")
    parser.add_argument(
        "--mode",
        choices=(*MOCK_ACTIONS, "ollama", "mcp", "combined", *PROBE_REPLIES),
        default="search",
        help="host response to exercise",
    )
    args = parser.parse_args()
    if not 0 <= args.budget_units < 2**63:
        parser.error("--budget-units must be a nonnegative SQLite integer")
    if not 0 <= args.budget_tokens < 2**63:
        parser.error("--budget-tokens must be a nonnegative SQLite integer")
    if not 0 <= args.budget_calls < 2**63:
        parser.error("--budget-calls must be a nonnegative SQLite integer")
    if args.mode in ("ollama", "combined") and not args.ollama_model:
        parser.error(f"--mode {args.mode} requires --ollama-model")
    if not 1 <= args.ollama_port <= 65535:
        parser.error("--ollama-port must be between 1 and 65535")
    if args.mode in ("mcp", "combined") and (
        args.mcp_port is None or not 1 <= args.mcp_port <= 65535
    ):
        parser.error(f"--mode {args.mode} requires --mcp-port between 1 and 65535")
    if not KERNEL.is_file():
        parser.error(f"build the RV64 release ELF from the Core repository first ({KERNEL})")

    if args.mode in PROBE_REPLIES:
        return run_qemu(PROBE_REPLIES[args.mode])

    model = None
    call_budget = None
    combined = args.mode == "combined"
    if args.mode in ("ollama", "combined"):
        model = args.ollama_model
        budget = args.budget_tokens
        budget_info = f"budget_tokens={budget} model={model}"
        selector = COMBINED_PLAN if combined else MODEL_GREETING
        run_model = partial(run_ollama_plan, model, args.ollama_port, combined=combined)
        services = {selector: Service(run_model, MAX_TOKENS)}
        if combined:
            call_budget = args.budget_calls
            budget_info += f" budget_calls={call_budget} mcp_port={args.mcp_port}"
            services[COMBINED_TOOL] = Service(partial(run_mcp_search, args.mcp_port), 1)
    elif args.mode == "mcp":
        budget = args.budget_calls
        budget_info = f"budget_calls={budget} mcp_port={args.mcp_port}"
        services = {MCP_SEARCH: Service(partial(run_mcp_search, args.mcp_port), 1)}
    else:
        budget = args.budget_units
        budget_info = f"budget_units={budget}"
        action = MOCK_ACTIONS[args.mode]

        def run_request(_operation_id: str) -> RequestResult | None:
            return None if action is None else RequestResult(Reply(action), MOCK_USED_UNITS)

        services = {MOCK_GREETING: Service(run_request, MOCK_MAX_UNITS)}

    path = args.journal.resolve()
    with closing(sqlite3.connect(path, isolation_level=None)) as db:
        journal = Journal(db)
        session = journal.create_session(budget, model, call_budget)
        print(
            f"host session {session} {budget_info} journal={path}",
            file=sys.stderr,
            flush=True,
        )

        def serve(peer, wire):
            serve_requests(peer, wire, journal, session, services)

        return run_qemu(b"PONG", serve)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, RuntimeError, sqlite3.Error) as error:
        print(f"host bridge: {error}", file=sys.stderr)
        raise SystemExit(1) from None
    except KeyboardInterrupt:
        raise SystemExit(130) from None

"""Serve and journal the recorded host replies for one combined greeting goal."""

from collections.abc import Callable
from contextlib import closing
from pathlib import Path
import re
import sqlite3
from typing import NamedTuple
from uuid import uuid4

from bridge_protocol import (
    BUDGET_EXHAUSTED, COMBINED_PLAN, COMBINED_TOOL, HostRequest, Reply, SEARCH_GREETING,
    SERVICE_UNAVAILABLE, SKIP, TOOL_OK, TOOL_RESULT_BYTES, valid_tool_text,
)
from ollama_backend import MAX_GENERATION_TOKENS, MAX_PROMPT_TOKENS, MAX_TOKENS


class RecordedReply(NamedTuple):
    request_id: str
    reply: Reply


class GoalReplay:
    def __init__(
        self, session: str, goal: str, launch: str, plan: RecordedReply, tool: RecordedReply | None
    ):
        self.session = session
        self.goal = goal
        self.launch = launch
        self.plan = plan
        self.tool = tool
        self.plan_request: HostRequest | None = None
        self.tool_request: HostRequest | None = None

    def reply(
        self, request: HostRequest, *, record: Callable[[HostRequest, RecordedReply], None]
    ) -> Reply:
        plan_request = self.plan_request
        if request == plan_request:
            return self.plan.reply
        if plan_request is None and request.selector == COMBINED_PLAN:
            record(request, self.plan)
            self.plan_request = request
            return self.plan.reply

        tool = self.tool
        if plan_request is None or tool is None:
            return Reply(SERVICE_UNAVAILABLE)
        if request == self.tool_request:
            return tool.reply

        # The fresh boot assigns its own handles. Bind the second reply to the
        # goal and launch that actually received the first reply.
        if (
            self.tool_request is None
            and request.selector == COMBINED_TOOL
            and request.goal_id == plan_request.goal_id
            and request.launch_id == plan_request.launch_id
            and request.request_id > plan_request.request_id
        ):
            record(request, tool)
            self.tool_request = request
            return tool.reply
        return Reply(SERVICE_UNAVAILABLE)


class ReplayJournal:
    def __init__(self, db: sqlite3.Connection, source: Path, replay: GoalReplay):
        self.db = db
        (mode,) = db.execute("PRAGMA journal_mode = WAL").fetchone()
        if mode != "wal":
            raise RuntimeError("could not enable WAL for the replay journal")
        db.executescript(f"""
            PRAGMA synchronous = FULL;
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS replay_sessions (
                id TEXT PRIMARY KEY NOT NULL,
                source_journal TEXT NOT NULL,
                source_session_id TEXT NOT NULL,
                source_goal_id TEXT NOT NULL,
                source_launch_id TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS replay_receipts (
                session_id TEXT NOT NULL REFERENCES replay_sessions(id),
                request_id TEXT NOT NULL,
                goal_id TEXT NOT NULL,
                launch_id TEXT NOT NULL,
                source_request_id TEXT NOT NULL,
                stage TEXT NOT NULL CHECK (stage IN ('model', 'tool')),
                reply INTEGER NOT NULL CHECK (reply BETWEEN 0 AND 255),
                result BLOB NOT NULL CHECK (length(result) <= {TOOL_RESULT_BYTES}),
                PRIMARY KEY (session_id, request_id),
                UNIQUE (session_id, source_request_id)
            );
        """)
        self.session = uuid4().hex
        with db:
            db.execute(
                """
                INSERT INTO replay_sessions
                    (id, source_journal, source_session_id, source_goal_id, source_launch_id)
                VALUES (?, ?, ?, ?, ?)
                """,
                (self.session, str(source), replay.session, replay.goal, replay.launch),
            )

    def record(self, request: HostRequest, recorded: RecordedReply) -> None:
        # Commit the selected reply before returning it to the wire handler.
        with self.db:
            self.db.execute(
                """
                INSERT INTO replay_receipts
                    (session_id, request_id, goal_id, launch_id, source_request_id, stage, reply, result)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                """,
                (
                    self.session, f"{request.request_id:016x}", f"{request.goal_id:016x}",
                    f"{request.launch_id:016x}", recorded.request_id,
                    "tool" if request.is_tool else "model", *recorded.reply,
                ),
            )


def journal_files(path: Path) -> set[Path | tuple[int, int]]:
    # SQLite uses adjacent files for its rollback journal and WAL.
    files: set[Path | tuple[int, int]] = set()
    for suffix in ("", "-journal", "-wal", "-shm"):
        entry = Path(f"{path}{suffix}").resolve()
        files.add(entry)
        try:
            stat = entry.stat()
        except FileNotFoundError:
            continue
        files.add((stat.st_dev, stat.st_ino))
    return files


def load_goal_replay(path: Path, session: str, goal: str) -> GoalReplay:
    session = session.lower()
    if not re.fullmatch(r"[0-9a-f]{32}", session):
        raise ValueError("replay session must be a 32-digit hexadecimal session ID")
    goal = goal.lower().removeprefix("0x")
    if not re.fullmatch(r"[0-9a-f]{1,16}", goal) or int(goal, 16) == 0:
        raise ValueError("replay goal must be a nonzero hexadecimal goal handle")
    goal = goal.zfill(16)

    uri = path.resolve().as_uri() + "?mode=ro"
    with closing(sqlite3.connect(uri, uri=True)) as db:
        db.row_factory = sqlite3.Row
        # Track result presence separately from the bounded payload projection.
        rows = db.execute("""
            SELECT o.request_id, o.launch_id, r.state, r.max_units, r.used_units, r.reply,
                   t.prompt_tokens, t.generation_tokens, c.goal_id AS tool_goal,
                   c.result IS NOT NULL AS has_result,
                   CASE WHEN length(c.result) <= ? THEN c.result END AS result
            FROM request_owners AS o
            JOIN model_sessions USING (session_id)
            JOIN mcp_budgets USING (session_id)
            LEFT JOIN requests AS r USING (session_id, request_id)
            LEFT JOIN token_usage AS t USING (session_id, request_id)
            LEFT JOIN mcp_calls AS c USING (session_id, request_id)
            WHERE o.session_id = ? AND o.goal_id = ?
            ORDER BY o.request_id
            LIMIT 3
        """, (TOOL_RESULT_BYTES, session, goal)).fetchall()

    if len(rows) not in (1, 2):
        raise ValueError("replay goal must have one or two recorded combined requests")
    plan, *remaining = rows
    launch = plan["launch_id"]
    plan_id = recorded_id(plan["request_id"])
    if not int(goal, 16) < recorded_id(launch) < plan_id:
        raise ValueError("replay requests have inconsistent ownership or order")

    plan_reply = RecordedReply(plan["request_id"], recorded_plan_reply(plan))
    if plan_reply.reply.status == SKIP:
        if remaining:
            raise ValueError("a recorded skip must be the only request for its goal")
        return GoalReplay(session, goal, launch, plan_reply, None)

    if not remaining:
        raise ValueError("a recorded search requires a host MCP request")
    (tool,) = remaining
    if launch != tool["launch_id"] or recorded_id(tool["request_id"]) <= plan_id:
        raise ValueError("replay requests have inconsistent ownership or order")
    tool_reply = RecordedReply(tool["request_id"], recorded_tool_reply(tool, goal))
    return GoalReplay(session, goal, launch, plan_reply, tool_reply)


def recorded_plan_reply(plan: sqlite3.Row) -> Reply:
    if plan["state"] != "settled":
        raise ValueError("replay requires a settled model request")
    prompt = plan["prompt_tokens"]
    generated = plan["generation_tokens"]
    status = plan["reply"]
    if not (
        plan["tool_goal"] is None
        and type(status) is int and status in (SEARCH_GREETING, SKIP)
        and plan["max_units"] == MAX_TOKENS
        and type(prompt) is int and 0 <= prompt <= MAX_PROMPT_TOKENS
        and type(generated) is int and 0 <= generated <= MAX_GENERATION_TOKENS
        and plan["used_units"] == prompt + generated
    ):
        raise ValueError("first replay request must contain a settled search or skip decision")
    return Reply(status)


def recorded_tool_reply(tool: sqlite3.Row, goal: str) -> Reply:
    state = tool["state"]
    if state not in ("settled", "denied"):
        raise ValueError("replay requires a resolved MCP request")
    status = tool["reply"]
    if not (
        tool["tool_goal"] == goal
        and type(status) is int
        and tool["max_units"] == 1
        and tool["prompt_tokens"] is None and tool["generation_tokens"] is None
    ):
        raise ValueError("recorded MCP request has inconsistent ownership or accounting")

    text = tool["result"]
    used = tool["used_units"]
    if state == "denied":
        if status != BUDGET_EXHAUSTED or used != 0 or tool["has_result"]:
            raise ValueError("recorded MCP denial must have a budget refusal, zero usage and no result")
        return Reply(status)

    if not (
        status == TOOL_OK and used == 1
        and isinstance(text, bytes) and valid_tool_text(text)
    ):
        raise ValueError("second replay request must be a settled MCP call with a valid result")
    return Reply(status, text)


def recorded_id(value: object) -> int:
    if not isinstance(value, str) or not re.fullmatch(r"[0-9a-f]{16}", value):
        raise ValueError("recorded replay handle must be a 16-digit hexadecimal ID")
    return int(value, 16)

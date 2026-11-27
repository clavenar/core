"""Read and serve the recorded host replies for one combined greeting goal."""

from contextlib import closing
from pathlib import Path
import re
import sqlite3

from bridge_protocol import (
    COMBINED_PLAN, COMBINED_TOOL, HostRequest, Reply, SEARCH_GREETING, SERVICE_UNAVAILABLE,
    TOOL_OK, TOOL_RESULT_BYTES, valid_tool_text,
)
from ollama_backend import MAX_GENERATION_TOKENS, MAX_PROMPT_TOKENS, MAX_TOKENS


class GoalReplay:
    def __init__(self, session: str, goal: str, launch: str, plan: Reply, tool: Reply):
        self.session = session
        self.goal = goal
        self.launch = launch
        self.plan_reply = plan
        self.tool_reply = tool
        self.plan_request: HostRequest | None = None
        self.tool_request: HostRequest | None = None

    def reply(self, request: HostRequest) -> Reply:
        if request == self.plan_request:
            return self.plan_reply
        if request == self.tool_request:
            return self.tool_reply

        plan = self.plan_request
        if plan is None and request.selector == COMBINED_PLAN:
            self.plan_request = request
            return self.plan_reply

        # The fresh boot assigns its own handles. Bind the second reply to the
        # goal and launch that actually received the first reply.
        if (
            plan is not None
            and self.tool_request is None
            and request.selector == COMBINED_TOOL
            and request.goal_id == plan.goal_id
            and request.launch_id == plan.launch_id
            and request.request_id > plan.request_id
        ):
            self.tool_request = request
            return self.tool_reply
        return Reply(SERVICE_UNAVAILABLE)


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
        rows = db.execute("""
            SELECT o.request_id, o.launch_id, r.state, r.max_units, r.used_units, r.reply,
                   t.prompt_tokens, t.generation_tokens, c.goal_id AS tool_goal,
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

    if len(rows) != 2:
        raise ValueError("replay goal must have exactly two recorded combined requests")
    plan, tool = rows
    if plan["state"] != "settled" or tool["state"] != "settled":
        raise ValueError("replay requires two settled requests")
    launch = plan["launch_id"]
    if launch != tool["launch_id"] or not (
        int(goal, 16) < recorded_id(launch) < recorded_id(plan["request_id"])
        < recorded_id(tool["request_id"])
    ):
        raise ValueError("replay requests have inconsistent ownership or order")

    prompt = plan["prompt_tokens"]
    generated = plan["generation_tokens"]
    plan_status = plan["reply"]
    if not (
        plan["tool_goal"] is None
        and type(plan_status) is int and plan_status == SEARCH_GREETING
        and plan["max_units"] == MAX_TOKENS
        and type(prompt) is int and 0 <= prompt <= MAX_PROMPT_TOKENS
        and type(generated) is int and 0 <= generated <= MAX_GENERATION_TOKENS
        and plan["used_units"] == prompt + generated
    ):
        raise ValueError("first replay request must be a settled model search decision")

    text = tool["result"]
    tool_status = tool["reply"]
    if not (
        tool["tool_goal"] == goal
        and type(tool_status) is int and tool_status == TOOL_OK
        and tool["max_units"] == tool["used_units"] == 1
        and tool["prompt_tokens"] is None and tool["generation_tokens"] is None
        and isinstance(text, bytes) and valid_tool_text(text)
    ):
        raise ValueError("second replay request must be a settled MCP call with a valid result")
    return GoalReplay(session, goal, launch, Reply(plan_status), Reply(tool_status, text))


def recorded_id(value: object) -> int:
    if not isinstance(value, str) or not re.fullmatch(r"[0-9a-f]{16}", value):
        raise ValueError("recorded replay handle must be a 16-digit hexadecimal ID")
    return int(value, 16)

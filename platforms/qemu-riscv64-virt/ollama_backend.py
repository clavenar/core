"""Ask a locally running model to choose one action for the greeting plan."""

from contextlib import closing
from http.client import HTTPConnection, HTTPException
import json
from typing import NamedTuple

MAX_PROMPT_TOKENS = 2048
MAX_GENERATION_TOKENS = 32
MAX_TOKENS = MAX_PROMPT_TOKENS + MAX_GENERATION_TOKENS
RESPONSE_BYTES = 16 * 1024
HTTP_TIMEOUT_SECONDS = 25
STATUS_TIMEOUT_SECONDS = 2

DECISION_SCHEMA = {
    "type": "object",
    "properties": {"action": {"type": "string", "enum": ["search", "skip"]}},
    "required": ["action"],
    "additionalProperties": False,
}
MESSAGES = [
    {
        "role": "system",
        "content": (
            "Choose the action that can satisfy the task. Exact text search is "
            "permitted on a granted file. Return a JSON action."
        ),
    },
    {
        "role": "user",
        "content": (
            "Goal: find lines containing Hello in the granted file. The allowed action "
            "search finds lines containing Hello in the granted file. The action skip "
            "ends the task without a search. Which action completes this goal?"
        ),
    },
]
HTTP_ERRORS = (OSError, HTTPException, ValueError, RecursionError)


class TokenUsage(NamedTuple):
    prompt_tokens: int
    generation_tokens: int


def choose_action(model: str, port: int) -> tuple[str | None, TokenUsage] | None:
    """Return a known decision and usage, or preserve an unknown outcome."""
    try:
        status = request_json(port, "/api/status", STATUS_TIMEOUT_SECONDS)
    except HTTP_ERRORS:
        return "unavailable", TokenUsage(0, 0)
    cloud = status.get("cloud") if isinstance(status, dict) else None
    if not isinstance(cloud, dict) or cloud.get("disabled") is not True:
        return "unavailable", TokenUsage(0, 0)

    try:
        reply = request_json(port, "/api/chat", HTTP_TIMEOUT_SECONDS, {
            "model": model,
            "messages": MESSAGES,
            "format": DECISION_SCHEMA,
            "stream": False,
            "think": False,
            "options": {
                "num_ctx": MAX_PROMPT_TOKENS,
                "num_predict": MAX_GENERATION_TOKENS,
                "temperature": 0,
            },
        })
    except HTTP_ERRORS:
        return None
    if (
        not isinstance(reply, dict)
        or reply.get("done") is not True
        or reply.get("remote_model")
        or reply.get("remote_host")
    ):
        return None

    prompt = reply.get("prompt_eval_count")
    generated = reply.get("eval_count")
    if not (
        type(prompt) is int
        and 0 <= prompt <= MAX_PROMPT_TOKENS
        and type(generated) is int
        and 0 <= generated <= MAX_GENERATION_TOKENS
    ):
        return None

    usage = TokenUsage(prompt, generated)
    return read_decision(reply), usage


def read_decision(reply: dict) -> str | None:
    message = reply.get("message")
    if reply.get("done_reason") != "stop" or not isinstance(message, dict):
        return None
    content = message.get("content")
    if not isinstance(content, str):
        return None
    try:
        decision = json.loads(content, object_pairs_hook=unique_fields)
    except (ValueError, RecursionError):
        return None
    if decision == {"action": "search"}:
        return "search"
    if decision == {"action": "skip"}:
        return "skip"
    return None


def request_json(port: int, path: str, timeout: int, body: dict | None = None) -> object:
    with closing(HTTPConnection("127.0.0.1", port, timeout=timeout)) as connection:
        if body is None:
            connection.request("GET", path)
        else:
            connection.request(
                "POST", path, json.dumps(body), {"Content-Type": "application/json"}
            )
        with connection.getresponse() as response:
            if response.status != 200:
                raise ValueError("Ollama returned an HTTP error")
            payload = response.read(RESPONSE_BYTES + 1)
    if len(payload) > RESPONSE_BYTES:
        raise ValueError("Ollama reply exceeds the response limit")
    return json.loads(payload.decode("utf-8"), object_pairs_hook=unique_fields)


def unique_fields(pairs: list[tuple[str, object]]) -> dict:
    result = dict(pairs)
    if len(result) != len(pairs):
        raise ValueError("duplicate JSON field")
    return result

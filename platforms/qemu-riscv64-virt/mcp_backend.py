"""Call a pinned, read-only search on a local MCP server."""

from contextlib import closing
from http.client import HTTPConnection
import json
from typing import NamedTuple

from host_json import HTTP_ERRORS, decode_json

PROTOCOL_VERSION = "2025-11-25"
ENDPOINT = "/mcp"
TOOL_NAME = "text.search"
PATTERN = "Hello"
TIMEOUT_SECONDS = 2
RESPONSE_BYTES = 4096
# 32-byte guest frame minus the 11-byte header and one status byte.
RESULT_BYTES = 20


class SearchResult(NamedTuple):
    used_calls: int
    text: bytes | None


def search_fixture(port: int, operation_id: str) -> SearchResult | None:
    with closing(HTTPConnection("127.0.0.1", port, timeout=TIMEOUT_SECONDS)) as connection:
        try:
            initialize(connection, operation_id)
        except HTTP_ERRORS:
            return SearchResult(0, None)

        # From this point a missing or uncorrelated reply leaves the call reserved.
        try:
            reply = post(connection, "tools/call", operation_id, {
                "name": TOOL_NAME,
                "arguments": {"pattern": PATTERN},
            })
        except HTTP_ERRORS:
            return None

    return SearchResult(1, None if reply is None else read_text(reply))


def initialize(connection: HTTPConnection, operation_id: str) -> None:
    reply = post(connection, "initialize", f"{operation_id}/initialize", {
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": {"name": "clavenar-host-bridge", "version": "1"},
    })
    if reply is None:
        raise ValueError("MCP server rejected initialization")
    capabilities = reply.get("capabilities")
    if (
        reply.get("protocolVersion") != PROTOCOL_VERSION
        or not isinstance(capabilities, dict)
        or not isinstance(capabilities.get("tools"), dict)
    ):
        raise ValueError("MCP server lacks the pinned protocol or tools capability")
    post(connection, "notifications/initialized")
    reply = post(connection, "tools/list", f"{operation_id}/tools")
    if reply is None or not approved_search(reply):
        raise ValueError("MCP server does not expose the pinned search schema")


def approved_search(reply: dict) -> bool:
    tools = reply.get("tools")
    if not isinstance(tools, list) or len(tools) != 1 or "nextCursor" in reply:
        return False
    (tool,) = tools
    if not isinstance(tool, dict) or tool.get("name") != TOOL_NAME:
        return False
    schema = tool.get("inputSchema")
    if not isinstance(schema, dict):
        return False
    properties = schema.get("properties")
    if not isinstance(properties, dict) or properties.keys() != {"pattern"}:
        return False
    pattern = properties["pattern"]
    return (
        {
            key: value for key, value in schema.items()
            if key not in ("title", "properties", "additionalProperties")
        }
        == {"type": "object", "required": ["pattern"]}
        and schema.get("additionalProperties", False) is False
        and isinstance(pattern, dict)
        and {key: value for key, value in pattern.items() if key != "title"} == {"type": "string"}
    )


def read_text(reply: dict) -> bytes | None:
    content = reply.get("content")
    if reply.get("isError", False) is not False or not isinstance(content, list) or len(content) != 1:
        return None
    (item,) = content
    if not isinstance(item, dict) or item.get("type") != "text":
        return None
    text = item.get("text")
    if not isinstance(text, str) or len(text) > RESULT_BYTES:
        return None
    if not all(" " <= char <= "~" for char in text.removesuffix("\n")):
        return None
    return text.encode("ascii")


def post(
    connection: HTTPConnection,
    method: str,
    request_id: str | None = None,
    params: dict | None = None,
) -> dict | None:
    headers = {"Accept": "application/json, text/event-stream", "Content-Type": "application/json"}
    if method != "initialize":
        headers["MCP-Protocol-Version"] = PROTOCOL_VERSION
    message = {"jsonrpc": "2.0", "method": method}
    if request_id is not None:
        message["id"] = request_id
    if params is not None:
        message["params"] = params
    connection.request("POST", ENDPOINT, json.dumps(message), headers)

    with connection.getresponse() as response:
        if response.getheader("Mcp-Session-Id") is not None:
            raise ValueError("expected a stateless MCP server")
        if request_id is None:
            if response.status == 202 and not response.read(1):
                return {}
            raise ValueError("MCP initialization notification was rejected")
        if response.status != 200 or response.headers.get_content_type() != "application/json":
            raise ValueError("expected a JSON MCP response")
        payload = response.read(RESPONSE_BYTES + 1)
        if len(payload) > RESPONSE_BYTES or not response.isclosed() or response.length:
            raise ValueError("MCP response is incomplete or exceeds the byte limit")
    reply = decode_json(payload)
    if (
        not isinstance(reply, dict)
        or reply.get("jsonrpc") != "2.0"
        or reply.get("id") != request_id
    ):
        raise ValueError("invalid MCP response envelope")
    if "error" not in reply and isinstance(reply.get("result"), dict):
        return reply["result"]
    error = reply.get("error")
    if (
        "result" not in reply
        and isinstance(error, dict)
        and type(error.get("code")) is int
        and isinstance(error.get("message"), str)
    ):
        return None
    raise ValueError("MCP response requires a result or an error")

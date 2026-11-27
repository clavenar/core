"""Bounded requests and replies on the QEMU VirtIO channel."""

import struct
from typing import NamedTuple

PLAN = struct.Struct("<BBBQB")
TOOL = struct.Struct("<BBBQBQ")
COMBINED = struct.Struct("<BBBQBQQ")
REQUEST_SIZES = (PLAN.size, TOOL.size, COMBINED.size)
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
TOOL_RESULT_BYTES = 32 - PLAN.size


class Reply(NamedTuple):
    status: int
    data: bytes = b""


class HostRequest(NamedTuple):
    request_id: int
    selector: int
    goal_id: int | None = None
    launch_id: int | None = None

    @property
    def is_tool(self) -> bool:
        return self.selector in (MCP_SEARCH, COMBINED_TOOL)


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


def encode_reply(request: HostRequest, reply: Reply) -> bytes:
    length = PLAN.size + len(reply.data)
    kind = TOOL_REPLY if request.is_tool else PLAN_REPLY
    return PLAN.pack(length, VERSION, kind, request.request_id, reply.status) + reply.data


def valid_tool_text(text: bytes) -> bool:
    return len(text) <= TOOL_RESULT_BYTES and all(
        32 <= byte <= 126 for byte in text.removesuffix(b"\n")
    )

"""Loopback MCP server for the QEMU walkthroughs."""

from mcp.server.fastmcp import FastMCP

server = FastMCP(
    "clavenar-fixture", host="127.0.0.1", port=8765,
    stateless_http=True, json_response=True,
)
fixture = ("Hello from MCP", "Goodbye from MCP")


@server.tool(name="text.search", structured_output=False)
def search(pattern: str) -> str:
    if pattern != "Hello":
        raise ValueError("fixture only accepts the Hello literal")
    print("tools/call text.search", flush=True)
    return next(line for line in fixture if pattern in line)


if __name__ == "__main__":
    server.run(transport="streamable-http")

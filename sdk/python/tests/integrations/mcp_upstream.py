"""A small upstream MCP server for the proxy tests (stdio)."""

import os

from mcp.server import MCPServer

server = MCPServer("upstream")
FILES = {"hr/salaries.csv": "nina: 120k, sam: 95k", "product/roadmap.md": "Q3: ship the new billing page"}
POSTED = []


@server.tool()
def read_file(path: str) -> str:
    """Read a file (or `env:NAME`, to show what the server was given)."""
    if path.startswith("env:"):
        return os.environ.get(path[4:], "")
    return FILES[path]


@server.tool()
def search(query: str) -> str:
    """Search everything."""
    return "found: nina: 120k"


@server.tool()
def post_message(text: str, to: list[str] | None = None) -> str:
    """Post a message."""
    POSTED.append(text)
    return f"posted ({len(POSTED)} so far)"


@server.tool()
def count_posts() -> str:
    """How many messages were posted."""
    return str(len(POSTED))


@server.resource("file:///hr/salaries.csv")
def salaries() -> str:
    return FILES["hr/salaries.csv"]


@server.resource("file:///product/roadmap.md")
def roadmap() -> str:
    return FILES["product/roadmap.md"]


if __name__ == "__main__":
    server.run()

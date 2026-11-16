"""A demo upstream MCP server: files to read and a channel to post to."""

from mcp.server import MCPServer

server = MCPServer("files")
FILES = {"hr/salaries.csv": "nina: 120k, sam: 95k", "product/roadmap.md": "Q3: ship the new billing page"}


@server.tool()
def read_file(path: str) -> str:
    """Read a file."""
    return FILES[path]


@server.tool()
def post_message(text: str) -> str:
    """Post to the engineering channel."""
    return "posted"


if __name__ == "__main__":
    server.run()

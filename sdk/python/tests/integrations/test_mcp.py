"""The MCP proxy against a real `trustgraph serve` and a real upstream MCP server over stdio."""

from __future__ import annotations

import asyncio
import os
import sys
from pathlib import Path
from typing import Any

import pytest

pytest.importorskip("mcp")
pytest.importorskip("yaml")

from mcp import Client as McpClient  # noqa: E402
from mcp.client.stdio import StdioServerParameters  # noqa: E402

from trustgraph.integrations.mcp import Proxy, load_mapping  # noqa: E402

from ..conftest import POLICIES, Server, start  # noqa: E402

UPSTREAM = StdioServerParameters(command=sys.executable, args=[str(Path(__file__).with_name("mcp_upstream.py"))])

MAPPING = """
sources:
  read_file:
    - source: salaries
      when: {path: "hr/*"}
    - source: roadmap
      when: {path: "product/*"}
actions:
  post_message:
    action: post_to_slack
    content: text
    recipients: to
resources:
  "file:///hr/*": salaries
"""


def mapping(tmp_path: Path, text: str = MAPPING, extra: str = "") -> Any:
    path = tmp_path / "mapping.yaml"
    path.write_text(text + extra)
    return load_mapping(str(path))


def text(result: Any) -> str:
    return result.content[0].text


async def with_proxy(server: Server, m: Any, script: Any, agent: str = "planner", **kw: Any) -> Any:
    async with Proxy(server.client(agent), m, UPSTREAM, **kw) as proxy_server, McpClient(proxy_server) as c:
        return await script(c)


def test_source_then_action_is_denied_and_not_forwarded(server: Server, tmp_path: Path) -> None:
    async def script(c: McpClient) -> None:
        assert text(await c.call_tool("read_file", {"path": "hr/salaries.csv"})) == "nina: 120k, sam: 95k"
        await c.call_tool("read_file", {"path": "product/roadmap.md"})
        denied = await c.call_tool("post_message", {"text": "Q3 plan", "to": ["sam"]})
        assert denied.is_error and text(denied).startswith("TrustGraph blocked `post_to_slack`: denied:")
        assert text(await c.call_tool("count_posts", {})) == "0"  # nothing was forwarded

    asyncio.run(with_proxy(server, mapping(tmp_path, extra="unmapped: allow\n"), script))

    async def allowed(c: McpClient) -> None:
        await c.call_tool("read_file", {"path": "product/roadmap.md"})
        assert text(await c.call_tool("post_message", {"text": "Q3", "to": ["sam"]})) == "posted (1 so far)"

    asyncio.run(with_proxy(server, mapping(tmp_path, extra="unmapped: block\n"), allowed))


def test_resources_are_sources(server: Server, tmp_path: Path) -> None:
    async def script(c: McpClient) -> None:
        listed = await c.list_resources()
        assert [str(r.uri) for r in listed.resources] == ["file:///hr/salaries.csv"]  # block hides the rest
        await c.read_resource("file:///hr/salaries.csv")
        assert (await c.call_tool("post_message", {"text": "x"})).is_error

    asyncio.run(with_proxy(server, mapping(tmp_path, extra="unmapped: block\n"), script))


def test_unmapped_block_hides_and_refuses(server: Server, tmp_path: Path) -> None:
    async def script(c: McpClient) -> None:
        names = {t.name for t in (await c.list_tools()).tools}
        assert names == {"read_file", "post_message"}
        refused = await c.call_tool("search", {"query": "salary"})
        assert refused.is_error and "not in the proxy's mapping" in text(refused)
        unmatched = await c.call_tool("read_file", {"path": "elsewhere.txt"})  # matchers fail: unmapped
        assert unmatched.is_error

    # The starter policy has no `unclassified` source, so the default is block.
    asyncio.run(with_proxy(server, mapping(tmp_path), script))


def test_unmapped_allow_forwards_untracked(server: Server, tmp_path: Path) -> None:
    async def script(c: McpClient) -> None:
        assert text(await c.call_tool("search", {"query": "salary"})) == "found: nina: 120k"
        assert (await c.call_tool("post_message", {"text": "x"})).is_error  # nothing tracked was read

    asyncio.run(with_proxy(server, mapping(tmp_path, extra="unmapped: allow\n"), script))


def test_unmapped_reads_carry_unclassified(tmp_path: Path) -> None:
    policy = POLICIES["starter"][0].read_text().replace(
        "sources:\n", "sources:\n  unclassified: { readers: [hr], agents: [planner] }\n", 1)
    server = start(tmp_path, "starter", policy)
    try:
        async def script(c: McpClient) -> None:
            await c.call_tool("read_file", {"path": "product/roadmap.md"})
            assert text(await c.call_tool("search", {"query": "salary"})) == "found: nina: 120k"
            denied = await c.call_tool("post_message", {"text": "x"})
            assert denied.is_error and "`unclassified` is readable by hr only" in text(denied)

        asyncio.run(with_proxy(server, mapping(tmp_path), script))
    finally:
        server.stop()


def test_handoff_file_and_carry(handoff_server: Server, tmp_path: Path) -> None:
    m = mapping(tmp_path, """
sources:
  read_file:
    - source: salaries
      when: {path: "hr/*"}
    - source: roadmap
      when: {path: "product/*"}
actions:
  post_message: post_to_engineering
""")
    handoff = tmp_path / "handoff.token"

    async def researcher(c: McpClient) -> None:
        await c.call_tool("read_file", {"path": "hr/salaries.csv"})

    asyncio.run(with_proxy(handoff_server, m, researcher, agent="researcher", handoff_file=str(handoff)))

    async def planner(c: McpClient) -> None:
        await c.call_tool("read_file", {"path": "product/roadmap.md"})
        denied = await c.call_tool("post_message", {"text": "Q3 summary"})
        assert denied.is_error and "`salaries` is readable by hr only" in text(denied)

    asyncio.run(with_proxy(handoff_server, m, planner, carry=handoff.read_text()))


def test_server_down_fails_closed(server: Server, tmp_path: Path) -> None:
    async def script(c: McpClient) -> None:
        await c.call_tool("read_file", {"path": "product/roadmap.md"})
        server.stop()
        withheld = await c.call_tool("read_file", {"path": "hr/salaries.csv"})
        assert withheld.is_error and "withheld" in text(withheld) and "120k" not in text(withheld)
        assert (await c.call_tool("post_message", {"text": "x"})).is_error

    asyncio.run(with_proxy(server, mapping(tmp_path, extra="unmapped: block\n"), script))


def test_bad_mapping_refuses_to_start(tmp_path: Path) -> None:
    for bad in ("sources: {t: [{when: {}}]}", "unmapped: maybe", "actions: {t: a}\nsources: {t: s}", "extra: 1"):
        path = tmp_path / "bad.yaml"
        path.write_text(bad)
        with pytest.raises(ValueError):
            load_mapping(str(path))


def test_the_command_over_stdio(server: Server, tmp_path: Path) -> None:
    (tmp_path / "mapping.yaml").write_text(MAPPING + "unmapped: block\n")
    proxy = StdioServerParameters(command=sys.executable, args=[
        "-m", "trustgraph.integrations.mcp", "--map", str(tmp_path / "mapping.yaml"),
        "--token-file", str(server.state / "tokens" / "planner.token"), "--url", server.url,
        "--", UPSTREAM.command, *UPSTREAM.args,
    ], env={"PATH": os.environ.get("PATH", "")})

    async def run() -> None:
        async with McpClient(proxy) as c:
            await c.call_tool("read_file", {"path": "hr/salaries.csv"})
            denied = await c.call_tool("post_message", {"text": "Q3 plan"})
            assert denied.is_error and "salaries" in text(denied)

    asyncio.run(run())

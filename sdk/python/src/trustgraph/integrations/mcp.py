"""An MCP proxy that checks tool calls and registers what tools and resources return.

    trustgraph-mcp-proxy --map mapping.yaml --token-file ~/.trustgraph/serve/tokens/planner.token \\
        -- npx some-mcp-server            # or: --upstream-url http://127.0.0.1:9000/mcp

It serves MCP on stdio to one client and forwards to one upstream server
(stdio command or streamable HTTP URL), with one TrustGraph session for the
life of the process. The upstream command gets only a minimal environment
plus the variables named with ``--pass-env``. The mapping file (YAML or JSON):

    sources:
      read_file:                      # a tool: its result is registered as a source
        - source: salaries
          when: {path: "hr/*"}        # optional argument matchers (glob on the value)
        - source: roadmap
    actions:
      post_message:                   # a tool: checked before it is forwarded
        action: post_to_engineering
        content: text                 # argument recorded as the output (default: all)
        recipients: to                # argument naming the recipients (default: whole audience)
    resources:
      "file:///hr/*": salaries        # resource URI globs
    unmapped: unclassified            # unclassified | allow | block

A mapped tool whose matchers all fail counts as unmapped. ``unmapped``
decides what happens to unmapped tools and resources: ``unclassified`` (the
default when the policy defines that source) forwards them and registers what
they return as source ``unclassified``; ``allow`` forwards them untracked;
``block`` (the default otherwise) hides and refuses them.
"""

from __future__ import annotations

import argparse
import asyncio
import fnmatch
import json
import os
import sys
from collections.abc import AsyncIterator, Mapping
from contextlib import asynccontextmanager
from dataclasses import dataclass, field
from typing import Any

import mcp.types as types
import yaml
from mcp import Client as McpClient
from mcp.client.stdio import StdioServerParameters
from mcp.server.lowlevel import Server
from mcp.server.stdio import stdio_server

from .._client import Client
from .._errors import TrustGraphError
from .._session import Session
from ._common import Action, blocked_message, not_registered_message

__all__ = ["ProxyMapping", "Proxy", "load_mapping", "main"]

UNCLASSIFIED = "unclassified"
MODES = ("unclassified", "allow", "block")


@dataclass(frozen=True)
class Rule:
    name: str  # source or action name
    when: Mapping[str, str] = field(default_factory=dict)
    content: str | None = None
    recipients: str | None = None

    def matches(self, args: Mapping[str, Any]) -> bool:
        return all(k in args and fnmatch.fnmatchcase(_text(args[k]), pattern) for k, pattern in self.when.items())


@dataclass
class ProxyMapping:
    """The parsed mapping file."""

    sources: dict[str, list[Rule]] = field(default_factory=dict)
    actions: dict[str, list[Rule]] = field(default_factory=dict)
    resources: dict[str, str] = field(default_factory=dict)
    unmapped: str | None = None  # None: decided from the policy

    def source_for(self, tool: str, args: Mapping[str, Any]) -> str | None:
        return next((r.name for r in self.sources.get(tool, ()) if r.matches(args)), None)

    def action_for(self, tool: str, args: Mapping[str, Any]) -> Rule | None:
        return next((r for r in self.actions.get(tool, ()) if r.matches(args)), None)

    def resource_source(self, uri: str) -> str | None:
        return next((s for pattern, s in self.resources.items() if fnmatch.fnmatchcase(uri, pattern)), None)


def _text(value: Any) -> str:
    return value if isinstance(value, str) else json.dumps(value, sort_keys=True)


def _rules(raw: Any, key: str) -> list[Rule]:
    items = raw if isinstance(raw, list) else [raw]
    rules = []
    for item in items:
        if isinstance(item, str):
            rules.append(Rule(item))
            continue
        if not isinstance(item, dict) or not isinstance(item.get(key), str):
            raise ValueError(f"each rule needs `{key}`: {item!r}")
        unknown = set(item) - {key, "when", "content", "recipients"}
        if unknown:
            raise ValueError(f"unknown rule keys {sorted(unknown)}")
        when = item.get("when") or {}
        if not isinstance(when, dict) or not all(isinstance(v, str) for v in when.values()):
            raise ValueError(f"`when` must map argument names to glob strings: {when!r}")
        rules.append(Rule(item[key], dict(when), item.get("content"), item.get("recipients")))
    return rules


def load_mapping(path: str) -> ProxyMapping:
    """Read a mapping file. Any problem raises ``ValueError``: the proxy does not start."""
    with open(path, encoding="utf-8") as f:
        raw = yaml.safe_load(f) or {}
    if not isinstance(raw, dict):
        raise ValueError("the mapping file must be a mapping")
    unknown = set(raw) - {"sources", "actions", "resources", "unmapped"}
    if unknown:
        raise ValueError(f"unknown keys {sorted(unknown)}")
    unmapped = raw.get("unmapped")
    if unmapped is not None and unmapped not in MODES:
        raise ValueError(f"`unmapped` must be one of {MODES}")
    m = ProxyMapping(
        sources={tool: _rules(r, "source") for tool, r in (raw.get("sources") or {}).items()},
        actions={tool: _rules(r, "action") for tool, r in (raw.get("actions") or {}).items()},
        resources=dict(raw.get("resources") or {}),
        unmapped=unmapped,
    )
    both = set(m.sources) & set(m.actions)
    if both:
        raise ValueError(f"a tool cannot be both a source and an action: {sorted(both)}")
    return m


def _error(text: str) -> types.CallToolResult:
    return types.CallToolResult(content=[types.TextContent(type="text", text=text)], is_error=True)


def _result_content(result: types.CallToolResult) -> Any:
    return result.model_dump(mode="json", include={"content", "structured_content", "is_error"}, exclude_none=True)


class Proxy:
    """The proxy: ``async with Proxy(...) as server`` yields the downstream MCP ``Server``."""

    def __init__(
        self,
        client: Client,
        mapping: ProxyMapping,
        upstream: StdioServerParameters | str | Any,
        *,
        carry: str | None = None,
        handoff_file: str | None = None,
    ) -> None:
        self.client = client
        self.mapping = mapping
        self.upstream = upstream
        self.handoff_file = handoff_file
        self.session: Session = client.session(carry=carry)
        self.unmapped = mapping.unmapped or self._default_unmapped()
        self._up: McpClient | None = None

    def _default_unmapped(self) -> str:
        try:
            policy = yaml.safe_load(self.client.policy()["policy"]) or {}
            return "unclassified" if UNCLASSIFIED in (policy.get("sources") or {}) else "block"
        except (TrustGraphError, yaml.YAMLError, KeyError, TypeError, AttributeError):
            return "block"

    # -- the session -----------------------------------------------------

    async def _read(self, source: str, content: Any) -> None:
        await asyncio.to_thread(self.session.read, source, content)
        if self.handoff_file:
            tmp = self.handoff_file + ".tmp"
            with open(tmp, "w", encoding="utf-8") as f:
                f.write(self.session.handoff())
            os.replace(tmp, self.handoff_file)

    # -- handlers --------------------------------------------------------

    def _visible(self, tool: str) -> bool:
        return self.unmapped != "block" or tool in self.mapping.sources or tool in self.mapping.actions

    async def list_tools(self, ctx: Any, params: types.PaginatedRequestParams | None) -> types.ListToolsResult:
        assert self._up is not None
        result = await self._up.list_tools(cursor=params.cursor if params else None, cache_mode="bypass")
        return types.ListToolsResult(tools=[t for t in result.tools if self._visible(t.name)],
                                     next_cursor=result.next_cursor)

    async def call_tool(self, ctx: Any, params: types.CallToolRequestParams) -> types.CallToolResult:
        assert self._up is not None
        name, args = params.name, dict(params.arguments or {})
        rule = self.mapping.action_for(name, args)
        source = self.mapping.source_for(name, args)
        if rule is not None:
            try:
                spec = Action(
                    rule.name,
                    content=(lambda a: a.get(rule.content)) if rule.content else None,
                    recipients=(lambda a: _names(a.get(rule.recipients))) if rule.recipients else None,
                )
                content = dict(args) if spec.content is None else spec.content(args)
                recipients = None if spec.recipients is None else spec.recipients(args)
                await asyncio.to_thread(self.session.act, spec.name, content, recipients)
            except TrustGraphError as e:
                return _error(blocked_message(e, rule.name))
            except Exception as e:  # fail closed
                return _error(blocked_message(TrustGraphError("internal_error", repr(e)), rule.name))
            return await self._up.call_tool(name, args)
        if source is None:
            if self.unmapped == "block":
                return _error(f"TrustGraph: tool `{name}` is not in the proxy's mapping. The tool did not run.")
            if self.unmapped == "allow":
                return await self._up.call_tool(name, args)
            source = UNCLASSIFIED
        result = await self._up.call_tool(name, args)
        try:
            await self._read(source, _result_content(result))
        except TrustGraphError as e:
            return _error(not_registered_message(source, e))
        except Exception as e:
            return _error(not_registered_message(source, TrustGraphError("internal_error", repr(e))))
        return result

    async def list_resources(self, ctx: Any, params: types.PaginatedRequestParams | None) -> types.ListResourcesResult:
        assert self._up is not None
        result = await self._up.list_resources(cursor=params.cursor if params else None, cache_mode="bypass")
        if self.unmapped == "block":
            result = types.ListResourcesResult(
                resources=[r for r in result.resources if self.mapping.resource_source(str(r.uri))],
                next_cursor=result.next_cursor)
        return result

    async def read_resource(self, ctx: Any, params: types.ReadResourceRequestParams) -> types.ReadResourceResult:
        assert self._up is not None
        uri = str(params.uri)
        source = self.mapping.resource_source(uri)
        if source is None:
            if self.unmapped == "block":
                raise ValueError(f"TrustGraph: resource `{uri}` is not in the proxy's mapping")
            if self.unmapped == "allow":
                return await self._up.read_resource(uri, cache_mode="bypass")
            source = UNCLASSIFIED
        result = await self._up.read_resource(uri, cache_mode="bypass")
        try:
            await self._read(source, result.model_dump(mode="json", include={"contents"}))
        except Exception as e:
            err = e if isinstance(e, TrustGraphError) else TrustGraphError("internal_error", repr(e))
            raise ValueError(not_registered_message(source, err)) from None
        return result

    @asynccontextmanager
    async def serve(self) -> AsyncIterator[Server[Any]]:
        async with McpClient(self.upstream, cache=None) as up:
            self._up = up
            try:
                yield Server(
                    "trustgraph-mcp-proxy",
                    on_list_tools=self.list_tools,
                    on_call_tool=self.call_tool,
                    on_list_resources=self.list_resources,
                    on_read_resource=self.read_resource,
                )
            finally:
                self._up = None

    async def __aenter__(self) -> Server[Any]:
        self._cm = self.serve()
        return await self._cm.__aenter__()

    async def __aexit__(self, *exc: Any) -> None:
        await self._cm.__aexit__(*exc)


def _names(value: Any) -> list[str] | None:
    if value is None:
        return None
    return [value] if isinstance(value, str) else [str(v) for v in value]


async def _run(args: argparse.Namespace) -> None:
    client = Client(args.url, token_file=args.token_file)
    mapping = load_mapping(args.map)
    if args.upstream_url:
        upstream: Any = args.upstream_url
    else:
        command = args.command[1:] if args.command[:1] == ["--"] else args.command
        if not command:
            raise SystemExit("give the upstream server: --upstream-url URL, or -- COMMAND [ARGS...]")
        missing = [name for name in args.pass_env if name not in os.environ]
        if missing:
            raise SystemExit(f"--pass-env: not set: {', '.join(missing)}")
        env = {name: os.environ[name] for name in args.pass_env}
        upstream = StdioServerParameters(command=command[0], args=command[1:], env=env or None)
    carry = args.carry or os.environ.get("TRUSTGRAPH_CARRY")
    proxy = Proxy(client, mapping, upstream, carry=carry, handoff_file=args.handoff_file)
    async with proxy as server, stdio_server() as (read, write):
        await server.run(read, write, server.create_initialization_options())


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(prog="trustgraph-mcp-proxy", description=__doc__.splitlines()[0])
    parser.add_argument("--map", required=True, help="mapping file (YAML or JSON)")
    parser.add_argument("--token-file", required=True, help="this agent's TrustGraph token file")
    parser.add_argument("--url", default="http://127.0.0.1:8787", help="trustgraph serve")
    parser.add_argument("--upstream-url", help="upstream MCP server over streamable HTTP")
    parser.add_argument("--pass-env", action="append", default=[], metavar="NAME",
                        help="pass this environment variable to the upstream command (repeatable)")
    parser.add_argument("--carry", help="a hand-off token to start from (or TRUSTGRAPH_CARRY)")
    parser.add_argument("--handoff-file", help="write this session's hand-off token here after every read")
    parser.add_argument("command", nargs=argparse.REMAINDER, help="-- the upstream server command")
    args = parser.parse_args(argv)
    try:
        asyncio.run(_run(args))
    except (ValueError, OSError, TrustGraphError) as e:
        print(f"trustgraph-mcp-proxy: {e}", file=sys.stderr)
        raise SystemExit(2) from None


if __name__ == "__main__":
    main()

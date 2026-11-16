"""Claude Agent SDK against a real `trustgraph serve`.

The Claude Code CLI would call a model, so a fake transport plays the CLI's
side of the SDK's control protocol: it receives the hook registration in the
initialize request and sends `hook_callback` requests, which the real
ClaudeSDKClient dispatches to the guard's callbacks.
"""

from __future__ import annotations

import asyncio
import itertools
import json
from collections.abc import AsyncIterator
from typing import Any

import pytest

pytest.importorskip("claude_agent_sdk")

from claude_agent_sdk import ClaudeAgentOptions, ClaudeSDKClient  # noqa: E402
from claude_agent_sdk._internal.transport import Transport  # noqa: E402

from trustgraph.integrations.claude_agent_sdk import Guard  # noqa: E402

from ..conftest import Server  # noqa: E402


class FakeCLI(Transport):
    def __init__(self) -> None:
        self.inbox: asyncio.Queue[dict[str, Any]] = asyncio.Queue()
        self.waiting: dict[str, asyncio.Future[dict[str, Any]]] = {}
        self.callbacks: dict[str, str] = {}
        self.ids = itertools.count()
        self.matchers: dict[str, Any] = {}

    async def connect(self) -> None:
        pass

    async def write(self, data: str) -> None:
        for line in data.splitlines():
            message = json.loads(line)
            if message.get("type") == "control_request" and message["request"]["subtype"] == "initialize":
                for event, matchers in (message["request"]["hooks"] or {}).items():
                    self.matchers[event] = matchers
                    self.callbacks[event] = matchers[0]["hookCallbackIds"][0]
                await self.inbox.put({"type": "control_response", "response": {
                    "subtype": "success", "request_id": message["request_id"],
                    "response": {"hook_on_failure_supported": True}}})
            elif message.get("type") == "control_response":
                self.waiting.pop(message["response"]["request_id"]).set_result(message["response"])

    async def read_messages(self) -> AsyncIterator[dict[str, Any]]:  # type: ignore[override]
        while True:
            message = await self.inbox.get()
            if message.get("type") == "end":
                return
            yield message

    async def close(self) -> None:
        await self.inbox.put({"type": "end"})

    def is_ready(self) -> bool:
        return True

    async def end_input(self) -> None:
        pass

    async def hook(self, event: str, data: dict[str, Any]) -> dict[str, Any]:
        """Fire a hook the way the CLI does and return the SDK's answer."""
        request_id = f"req_{next(self.ids)}"
        future: asyncio.Future[dict[str, Any]] = asyncio.get_running_loop().create_future()
        self.waiting[request_id] = future
        await self.inbox.put({"type": "control_request", "request_id": request_id, "request": {
            "subtype": "hook_callback", "callback_id": self.callbacks[event],
            "input": {"hook_event_name": event, "session_id": "s1", "cwd": "/", "transcript_path": "/t", **data},
            "tool_use_id": data.get("tool_use_id")}})
        answer = await asyncio.wait_for(future, 30)
        assert answer["subtype"] == "success", answer
        return answer["response"]


def tool(name: str, args: dict[str, Any] | None = None, response: Any = None, **extra: Any) -> dict[str, Any]:
    data = {"tool_name": name, "tool_input": args or {}, "tool_use_id": "t", **extra}
    if response is not None:
        data["tool_response"] = response
    return data


def denied(answer: dict[str, Any]) -> str | None:
    out = answer.get("hookSpecificOutput") or {}
    return out.get("permissionDecisionReason") if out.get("permissionDecision") == "deny" else None


async def drive(guard: Guard, script: Any) -> Any:
    cli = FakeCLI()
    async with ClaudeSDKClient(ClaudeAgentOptions(hooks=guard.hooks()), transport=cli):
        assert all(m[0]["onFailure"] == "block" for m in cli.matchers.values())
        return await script(cli)


SALARIES = {"content": [{"type": "text", "text": "nina: 120k, sam: 95k"}]}
ROADMAP = {"content": [{"type": "text", "text": "Q3: ship the new billing page"}]}


def planner_guard(server: Server, **kw: Any) -> Guard:
    return Guard(server.client("planner"),
                 sources={"mcp__drive__salaries": "salaries", "mcp__drive__roadmap": "roadmap"},
                 actions={"mcp__slack__post": "post_to_slack"}, **kw)


def test_source_then_action_is_denied(server: Server) -> None:
    async def script(cli: FakeCLI) -> None:
        assert await cli.hook("PostToolUse", tool("mcp__drive__salaries", response=SALARIES)) == {}
        reason = denied(await cli.hook("PreToolUse", tool("mcp__slack__post", {"text": "Q3 plan"})))
        assert reason and reason.startswith("TrustGraph blocked `post_to_slack`: denied:") and "salaries" in reason
        assert await cli.hook("PreToolUse", tool("Read", {"file_path": "/x"})) == {}  # unmapped

    asyncio.run(drive(planner_guard(server), script))


def test_allow_returns_no_opinion(server: Server) -> None:
    async def script(cli: FakeCLI) -> None:
        await cli.hook("PostToolUse", tool("mcp__drive__roadmap", response=ROADMAP))
        assert await cli.hook("PreToolUse", tool("mcp__slack__post", {"text": "Q3"})) == {}

    asyncio.run(drive(planner_guard(server), script))


def test_needs_approval_is_a_deny_with_the_decision(server: Server) -> None:
    guard = Guard(server.client("mailer"), sources={"mcp__sheets__prices": "price_sheet"},
                  actions={"mcp__mail__send": "email_customer"})

    async def script(cli: FakeCLI) -> None:
        await cli.hook("PostToolUse", tool("mcp__sheets__prices", response="Widget: $12"))
        reason = denied(await cli.hook("PreToolUse", tool("mcp__mail__send", {"to": "acme"})))
        assert reason and reason.startswith("TrustGraph held `email_customer`: needs approval") and "decision " in reason

    asyncio.run(drive(guard, script))


def test_subagent_carries_parent_and_returns_to_it(handoff_server: Server) -> None:
    guard = Guard(handoff_server.client("planner"),
                  sources={"mcp__drive__salaries": "salaries", "mcp__drive__roadmap": "roadmap"},
                  actions={"mcp__slack__post": "post_to_engineering"},
                  subagents={"researcher": handoff_server.client("researcher")})
    sub = {"agent_id": "a1", "agent_type": "researcher"}

    async def script(cli: FakeCLI) -> None:
        await cli.hook("SubagentStart", sub)
        await cli.hook("PostToolUse", tool("mcp__drive__salaries", response=SALARIES, **sub))
        await cli.hook("SubagentStop", {**sub, "stop_hook_active": False, "agent_transcript_path": "/t"})
        await cli.hook("PostToolUse", tool("mcp__drive__roadmap", response=ROADMAP))
        reason = denied(await cli.hook("PreToolUse", tool("mcp__slack__post", {"text": "Q3 summary"})))
        assert reason and "`salaries` is readable by hr only" in reason

    asyncio.run(drive(guard, script))


def test_subagent_not_cleared_for_parent_is_refused(handoff_server: Server) -> None:
    guard = Guard(handoff_server.client("planner"), sources={"mcp__drive__roadmap": "roadmap"},
                  actions={"mcp__slack__post": "post_to_engineering"},
                  subagents={"researcher": handoff_server.client("researcher")})
    sub = {"agent_id": "a1", "agent_type": "researcher"}

    async def script(cli: FakeCLI) -> None:
        await cli.hook("PostToolUse", tool("mcp__drive__roadmap", response=ROADMAP))
        request_id = "start"
        future = asyncio.get_running_loop().create_future()
        cli.waiting[request_id] = future
        await cli.inbox.put({"type": "control_request", "request_id": request_id, "request": {
            "subtype": "hook_callback", "callback_id": cli.callbacks["SubagentStart"],
            "input": {"hook_event_name": "SubagentStart", "session_id": "s1", **sub}}})
        assert (await future)["subtype"] == "error"  # with on_failure="block" the subagent does not start
        reason = denied(await cli.hook("PreToolUse", tool("mcp__slack__post", {"text": "x"}, **sub)))
        assert reason and "session_broken" in reason

    asyncio.run(drive(guard, script))


def test_server_down_fails_closed(server: Server) -> None:
    async def script(cli: FakeCLI) -> None:
        await cli.hook("PostToolUse", tool("mcp__drive__roadmap", response=ROADMAP))
        server.stop()
        reason = denied(await cli.hook("PreToolUse", tool("mcp__slack__post", {"text": "x"})))
        assert reason and "unreachable" in reason
        answer = await cli.hook("PostToolUse", tool("mcp__drive__salaries", response=SALARIES))
        assert answer["continue"] is False
        assert "withheld" in answer["hookSpecificOutput"]["updatedToolOutput"]

    asyncio.run(drive(planner_guard(server), script))

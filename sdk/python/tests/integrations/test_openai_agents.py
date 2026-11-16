"""OpenAI Agents SDK against a real `trustgraph serve`. No model API: the SDK's ScriptedModel."""

from __future__ import annotations

import asyncio
import json
from typing import Any

import pytest

pytest.importorskip("agents")

from agents import Agent, Runner, function_tool, set_tracing_disabled  # noqa: E402
from agents.items import ToolCallOutputItem  # noqa: E402
from agents.testing import ScriptedModel, assistant_message, function_call  # noqa: E402

from trustgraph import TrustGraphError  # noqa: E402
from trustgraph.integrations.openai_agents import Action, Guard, as_action, as_source  # noqa: E402

from ..conftest import Server  # noqa: E402
from ..test_server import recording  # noqa: E402

set_tracing_disabled(True)  # nothing leaves the machine
posted: list[str] = []


@pytest.fixture(autouse=True)
def _clear() -> None:
    posted.clear()


@function_tool
def read_salaries() -> str:
    """Read the HR salary file."""
    return "nina: 120k, sam: 95k"


@function_tool
def read_roadmap() -> str:
    """Read the product roadmap."""
    return "Q3: ship the new billing page"


@function_tool
def read_prices() -> str:
    """Read the price sheet."""
    return "Widget: $12"


@function_tool
def post_message(text: str) -> str:
    """Post to the team channel."""
    posted.append(text)
    return "posted"


@function_tool
def brief_hr(text: str) -> str:
    """Brief HR."""
    posted.append("hr: " + text)
    return "briefed"


def script(*steps: list) -> ScriptedModel:
    return ScriptedModel([*steps, [assistant_message("done")]])


def call(name: str, args: dict | None = None, n: int = 0) -> list:
    return [function_call(name, args or {}, call_id=f"{name}-{n}")]


def outputs(result) -> dict[str, str]:
    return {item.raw_item["call_id"].rsplit("-", 1)[0]: str(item.output)
            for item in result.new_items if isinstance(item, ToolCallOutputItem)}


def test_wrapped_tools_deny_and_allow(server: Server) -> None:
    tools = [as_source(read_salaries, "salaries"), as_source(read_roadmap, "roadmap"),
             as_action(post_message, Action("post_to_slack", content=lambda a: a["text"]))]
    agent = Agent(name="planner", tools=tools,
                  model=script(call("read_salaries"), call("read_roadmap"), call("post_message", {"text": "Q3 plan"})))
    planner, bodies = recording(server, "planner")
    with planner.session():
        result = Runner.run_sync(agent, "post the plan")
    out = outputs(result)
    assert bodies and "nina" not in json.dumps(bodies) and "Q3 plan" not in json.dumps(bodies)  # hashes only
    assert out["read_salaries"] == "nina: 120k, sam: 95k"
    assert out["post_message"].startswith("TrustGraph blocked `post_to_slack`: denied:") and "salaries" in out["post_message"]
    assert posted == []

    agent = Agent(name="planner", tools=tools, model=script(call("read_roadmap"), call("post_message", {"text": "Q3"})))
    with server.client("planner").session():
        assert outputs(Runner.run_sync(agent, "post"))["post_message"] == "posted"
    assert posted == ["Q3"]


def test_no_session_withholds_and_blocks(server: Server) -> None:
    agent = Agent(name="planner", tools=[as_source(read_salaries, "salaries"), as_action(post_message, "post_to_slack")],
                  model=script(call("read_salaries"), call("post_message", {"text": "x"})))
    out = outputs(Runner.run_sync(agent, "go"))
    assert "withheld" in out["read_salaries"] and "120k" not in out["read_salaries"]
    assert "did not run" in out["post_message"] and posted == []


def test_needs_approval_is_distinct(server: Server) -> None:
    guard = Guard(clients={"mailer": server.client("mailer")},
                  sources={"read_prices": "price_sheet"}, actions={"post_message": "email_customer"})
    agent = Agent(name="mailer", tools=[read_prices, post_message],
                  model=script(call("read_prices"), call("post_message", {"text": "Dear Acme"})))
    guard.protect(agent)
    out = outputs(guard.run_sync(agent, "quote"))
    assert out["post_message"].startswith("TrustGraph held `email_customer`: needs approval")
    assert posted == []


def handoff_agents(server: Server, first: str, first_tool: str, second: str, second_steps: list) -> tuple[Guard, Agent]:
    two = Agent(name=second, tools=[read_salaries, read_roadmap, post_message, brief_hr], model=script(*second_steps))
    one = Agent(name=first, tools=[read_salaries, read_roadmap], handoffs=[two],
                model=ScriptedModel([call(first_tool), call(f"transfer_to_{second}")]))
    guard = Guard(
        clients={first: server.client(first), second: server.client(second)},
        sources={"read_salaries": "salaries", "read_roadmap": "roadmap"},
        actions={"post_message": "post_to_engineering", "brief_hr": "brief_hr"},
    )
    guard.protect(one, two)
    return guard, one


def test_handoff_carries_the_session(handoff_server: Server) -> None:
    guard, researcher = handoff_agents(handoff_server, "researcher", "read_salaries", "planner", [
        call("read_roadmap"), call("post_message", {"text": "Q3 summary"}), call("brief_hr", {"text": "Q3 summary"}),
    ])
    result = asyncio.run(guard.run(researcher, "plan Q3"))
    out = outputs(result)
    assert result.last_agent.name == "planner"
    assert "`salaries` is readable by hr only" in out["post_message"]
    assert out["brief_hr"] == "briefed" and posted == ["hr: Q3 summary"]


def test_handoff_to_an_uncleared_agent_stops_the_run(handoff_server: Server) -> None:
    guard, planner = handoff_agents(handoff_server, "planner", "read_roadmap", "researcher", [])
    with pytest.raises(TrustGraphError) as e:
        guard.run_sync(planner, "go")
    assert e.value.code == "unknown_artifact"


def test_server_down_fails_closed(server: Server) -> None:
    guard = Guard(clients={"planner": server.client("planner")},
                  sources={"read_roadmap": "roadmap"}, actions={"post_message": "post_to_slack"})
    agent = Agent(name="planner", tools=[read_roadmap, post_message],
                  model=script(call("read_roadmap"), call("post_message", {"text": "x"})))
    guard.protect(agent)
    server.stop()
    out = outputs(guard.run_sync(agent, "go"))
    assert "unreachable" in out["read_roadmap"] and "withheld" in out["read_roadmap"]
    assert "did not run" in out["post_message"] and posted == []


def test_unmapped_agent_is_refused(server: Server) -> None:
    guard = Guard(clients={}, actions={"post_message": "post_to_slack"})
    agent = Agent(name="stranger", tools=[post_message], model=script(call("post_message", {"text": "x"})))
    guard.protect(agent)
    out = outputs(guard.run_sync(agent, "go"))
    assert "no TrustGraph client for agent `stranger`" in out["post_message"] and posted == []


# -- tools from Agent.mcp_servers -------------------------------------------


class FakeMCP:
    """An in-process MCP server (the SDK's MCPServer interface), no subprocess."""

    def __new__(cls) -> Any:
        from agents.mcp import MCPServer
        from mcp.types import CallToolResult, TextContent, Tool

        class Server(MCPServer):
            async def connect(self) -> None:
                pass

            @property
            def name(self) -> str:
                return "drive"

            async def cleanup(self) -> None:
                pass

            async def list_tools(self, run_context: Any = None, agent: Any = None) -> list[Any]:
                return [Tool(name="mcp_salaries", inputSchema={"type": "object", "properties": {}}),
                        Tool(name="mcp_post", inputSchema={"type": "object",
                                                           "properties": {"text": {"type": "string"}}})]

            async def call_tool(self, tool_name: str, arguments: Any, meta: Any = None) -> Any:
                if tool_name == "mcp_post":
                    posted.append(arguments["text"])
                    return CallToolResult(content=[TextContent(type="text", text="posted")])
                return CallToolResult(content=[TextContent(type="text", text="nina: 120k, sam: 95k")])

            async def list_prompts(self) -> Any:
                raise NotImplementedError

            async def get_prompt(self, name: str, arguments: Any = None) -> Any:
                raise NotImplementedError

        return Server()


def test_mcp_server_tools_are_guarded(server: Server) -> None:
    guard = Guard(clients={"planner": server.client("planner")},
                  sources={"mcp_salaries": "salaries"}, actions={"mcp_post": "post_to_slack"})
    agent = Agent(name="planner", mcp_servers=[FakeMCP()],
                  model=script(call("mcp_salaries"), call("mcp_post", {"text": "Q3 plan"})))
    guard.protect(agent)
    guard.protect(agent)  # idempotent
    assert len(agent.mcp_servers[0].tool_input_guardrails) == 1
    out = outputs(guard.run_sync(agent, "go"))
    assert "nina: 120k" in out["mcp_salaries"]
    assert out["mcp_post"].startswith("TrustGraph blocked `post_to_slack`: denied:") and posted == []


# -- conversations that outlive one run -------------------------------------


def memory_agents(server: Server, steps: list) -> tuple[Guard, Agent]:
    guard = Guard(clients={"planner": server.client("planner")},
                  sources={"read_salaries": "salaries", "read_roadmap": "roadmap"},
                  actions={"post_message": "post_to_slack"})
    agent = Agent(name="planner", tools=[read_salaries, read_roadmap, post_message], model=script(*steps))
    guard.protect(agent)
    return guard, agent


def test_resumed_memory_session_without_the_token_is_refused(server: Server) -> None:
    from agents import SQLiteSession

    memory = SQLiteSession("conversation")
    guard, agent = memory_agents(server, [call("read_salaries")])
    context: dict[str, Any] = {}
    guard.run_sync(agent, "read", session=memory, context=context)
    assert context["trustgraph"].startswith("tg1.")

    guard, agent = memory_agents(server, [call("read_roadmap"), call("post_message", {"text": "x"}, 1)])
    out = outputs(guard.run_sync(agent, "post", session=memory))  # history, but a fresh context
    assert "continues an earlier conversation" in out["post_message"] and posted == []

    guard, agent = memory_agents(server, [call("read_roadmap"), call("post_message", {"text": "x"}, 1)])
    out = outputs(guard.run_sync(agent, "post", session=memory, context=context))  # restored from it
    assert "`salaries`" in out["post_message"] and "denied" in out["post_message"] and posted == []


def test_input_with_earlier_tool_items_is_refused(server: Server) -> None:
    history = [{"role": "user", "content": "read the salaries"},
               {"type": "function_call", "call_id": "c0", "name": "read_salaries", "arguments": "{}"},
               {"type": "function_call_output", "call_id": "c0", "output": "nina: 120k, sam: 95k"},
               {"role": "user", "content": "now post the plan"}]
    guard, agent = memory_agents(server, [call("read_roadmap"), call("post_message", {"text": "x"})])
    out = outputs(guard.run_sync(agent, history))
    assert "continues an earlier conversation" in out["post_message"] and posted == []
    guard, agent = memory_agents(server, [call("read_roadmap"), call("post_message", {"text": "x"})])
    assert outputs(guard.run_sync(agent, [{"role": "user", "content": "post"}]))["post_message"] == "posted"


def test_run_streamed_is_scoped(server: Server) -> None:
    guard, agent = memory_agents(server, [call("read_salaries"), call("post_message", {"text": "x"})])

    async def run() -> Any:
        result = await guard.run_streamed(agent, "go")
        async for _ in result.stream_events():
            pass
        return result

    out = outputs(asyncio.run(run()))
    assert "`salaries`" in out["post_message"] and posted == []


def test_runstate_resume_restores_or_refuses(server: Server) -> None:
    """A run interrupted for the SDK's own tool approval, serialised and resumed."""
    from agents import RunState

    @function_tool(needs_approval=True)
    def publish(text: str) -> str:
        """Publish, after a person approves."""
        posted.append(text)
        return "published"

    def agent() -> tuple[Guard, Agent]:
        guard = Guard(clients={"planner": server.client("planner")},
                      sources={"read_salaries": "salaries"}, actions={"publish": "post_to_slack"})
        a = Agent(name="planner", tools=[read_salaries, publish],
                  model=script(call("read_salaries"), call("publish", {"text": "x"})))
        guard.protect(a)
        return guard, a

    for context in ({}, None):
        guard, a = agent()
        result = guard.run_sync(a, "go", context=context)
        assert result.interruptions and posted == []
        saved = result.to_state().to_json()
        guard, a = agent()  # a new process
        state = asyncio.run(RunState.from_json(a, saved))
        state.approve(state.get_interruptions()[0])
        out = outputs(guard.run_sync(a, state))
        assert posted == []
        if context is not None:  # the dict context travelled in the RunState with the token
            assert "denied" in out["publish"] and "`salaries`" in out["publish"]
        else:
            assert "continues an earlier conversation" in out["publish"]

"""OpenAI Agents SDK against a real `trustgraph serve`. No model API: the SDK's ScriptedModel."""

from __future__ import annotations

import asyncio

import pytest

pytest.importorskip("agents")

from agents import Agent, Runner, function_tool, set_tracing_disabled  # noqa: E402
from agents.items import ToolCallOutputItem  # noqa: E402
from agents.testing import ScriptedModel, assistant_message, function_call  # noqa: E402

from trustgraph import TrustGraphError  # noqa: E402
from trustgraph.integrations.openai_agents import Action, Guard, as_action, as_source  # noqa: E402

from ..conftest import Server  # noqa: E402

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
    with server.client("planner").session():
        result = Runner.run_sync(agent, "post the plan")
    out = outputs(result)
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

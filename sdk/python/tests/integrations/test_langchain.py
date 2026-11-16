"""LangChain / LangGraph against a real `trustgraph serve`. No model API: a scripted fake model."""

from __future__ import annotations

import asyncio
from typing import Any

import pytest

pytest.importorskip("langgraph")
pytest.importorskip("langchain")

from langchain.agents import create_agent  # noqa: E402
from langchain_core.language_models.fake_chat_models import GenericFakeChatModel  # noqa: E402
from langchain_core.messages import AIMessage, HumanMessage, ToolMessage  # noqa: E402
from langchain_core.tools import tool  # noqa: E402
from langgraph.checkpoint.memory import InMemorySaver  # noqa: E402
from langgraph.graph import END, START, MessagesState, StateGraph  # noqa: E402
from langgraph.prebuilt import ToolNode  # noqa: E402

import json  # noqa: E402

from trustgraph import TrustGraphError  # noqa: E402
from trustgraph.integrations.langchain import Action, Guard, agent_node, as_action, as_source  # noqa: E402

from ..conftest import Server  # noqa: E402
from ..test_server import recording  # noqa: E402

posted: list[str] = []


@tool
def read_salaries() -> str:
    """Read the HR salary file."""
    return "nina: 120k, sam: 95k"


@tool
def read_roadmap() -> str:
    """Read the product roadmap."""
    return "Q3: ship the new billing page"


@tool
def post_message(text: str) -> str:
    """Post to the engineering channel."""
    posted.append(text)
    return "posted"


@pytest.fixture(autouse=True)
def _clear() -> None:
    posted.clear()


def calls(*names_args: tuple[str, dict[str, Any]]) -> AIMessage:
    return AIMessage("", tool_calls=[{"name": n, "args": a, "id": f"call_{i}"} for i, (n, a) in enumerate(names_args)])


def graph_of(node: ToolNode) -> Any:
    """A one-node graph: ToolNode needs the graph runtime."""
    g = StateGraph(MessagesState)
    g.add_node("tools", node)
    g.add_edge(START, "tools")
    g.add_edge("tools", END)
    return g.compile()


def run_tools(node: ToolNode, message: AIMessage, config: dict[str, Any] | None = None) -> list[ToolMessage]:
    return graph_of(node).invoke({"messages": [message]}, config or {})["messages"][1:]


class ScriptedModel(GenericFakeChatModel):
    """A fake chat model that plays back fixed messages and accepts tools."""

    def bind_tools(self, tools: Any, **kwargs: Any) -> ScriptedModel:
        return self


def test_wrapped_tools_in_a_tool_node(server: Server) -> None:
    node = ToolNode([as_source(read_salaries, "salaries"), as_source(read_roadmap, "roadmap"),
                     as_action(post_message, Action("post_to_slack", content=lambda a: a["text"]))])
    planner, bodies = recording(server, "planner")
    with planner.session():
        [salaries, roadmap] = run_tools(node, calls(("read_salaries", {}), ("read_roadmap", {})))
        assert salaries.content == "nina: 120k, sam: 95k"
        [blocked] = run_tools(node, calls(("post_message", {"text": "Q3 plan"})))
    assert blocked.status == "error"
    assert "TrustGraph blocked `post_to_slack`: denied:" in blocked.content and "salaries" in blocked.content
    assert posted == []
    assert bodies and "nina" not in json.dumps(bodies) and "Q3 plan" not in json.dumps(bodies)  # hashes only

    with server.client("planner").session():
        run_tools(node, calls(("read_roadmap", {})))
        [ok] = run_tools(node, calls(("post_message", {"text": "Q3: billing"})))
    assert ok.content == "posted" and posted == ["Q3: billing"]


def test_wrapped_tools_without_a_session_are_refused(server: Server) -> None:
    node = ToolNode([as_source(read_salaries, "salaries"), as_action(post_message, "post_to_slack")])
    [read, post] = run_tools(node, calls(("read_salaries", {}), ("post_message", {"text": "x"})))
    assert read.status == "error" and "withheld" in read.content and "120k" not in read.content
    assert post.status == "error" and posted == []


def test_guard_on_a_tool_node_with_needs_approval(server: Server) -> None:
    @tool
    def read_prices() -> str:
        """Read the price sheet."""
        return "Widget: $12"

    @tool
    def send_email(text: str) -> str:
        """Email the customer."""
        posted.append(text)
        return "sent"

    guard = Guard(sources={"read_prices": "price_sheet"}, actions={"send_email": "email_customer"})
    node = ToolNode([read_prices, send_email], wrap_tool_call=guard.wrap_tool_call)
    with server.client("mailer").session():
        run_tools(node, calls(("read_prices", {})))
        [msg] = run_tools(node, calls(("send_email", {"text": "Dear Acme"})))
    assert msg.status == "error" and "needs approval" in msg.content and "decision " in msg.content
    assert posted == []


def test_guard_per_thread_sessions(server: Server) -> None:
    guard = Guard(sources={"read_salaries": "salaries", "read_roadmap": "roadmap"},
                  actions={"post_message": "post_to_slack"}, client=server.client("planner"))
    node = ToolNode([read_salaries, read_roadmap, post_message], wrap_tool_call=guard.wrap_tool_call)
    a = {"configurable": {"thread_id": "a"}}
    b = {"configurable": {"thread_id": "b"}}
    run_tools(node, calls(("read_salaries", {})), a)
    run_tools(node, calls(("read_roadmap", {})), b)
    [denied] = run_tools(node, calls(("post_message", {"text": "x"})), a)  # thread a read salaries earlier
    [allowed] = run_tools(node, calls(("post_message", {"text": "y"})), b)
    assert denied.status == "error" and allowed.content == "posted" and posted == ["y"]
    [refused] = run_tools(node, calls(("post_message", {"text": "z"})))  # no thread, no session
    assert refused.status == "error" and posted == ["y"]


def test_create_agent_middleware(server: Server) -> None:
    guard = Guard(sources={"read_salaries": "salaries"}, actions={"post_message": "post_to_slack"})
    model = ScriptedModel(messages=iter([
        calls(("read_salaries", {})),
        calls(("post_message", {"text": "Q3 headcount plan"})),
        AIMessage("I could not post it: the plan carries salary data."),
    ]))
    agent = create_agent(model, [read_salaries, post_message], middleware=[guard.middleware()],
                         checkpointer=InMemorySaver())
    with server.client("planner").session():
        out = agent.invoke({"messages": [HumanMessage("post the plan")]}, {"configurable": {"thread_id": "1"}})
    tool_messages = [m for m in out["messages"] if isinstance(m, ToolMessage)]
    assert tool_messages[1].status == "error" and "salaries" in tool_messages[1].content
    assert posted == []


def test_async_tool_node(server: Server) -> None:
    guard = Guard(sources={"read_salaries": "salaries"}, actions={"post_message": "post_to_slack"})
    node = ToolNode([read_salaries, post_message], awrap_tool_call=guard.awrap_tool_call)

    async def run() -> list[ToolMessage]:
        async with server.client("planner").session():
            g = graph_of(node)
            await g.ainvoke({"messages": [calls(("read_salaries", {}))]})
            return (await g.ainvoke({"messages": [calls(("post_message", {"text": "x"}))]}))["messages"][1:]

    [msg] = asyncio.run(run())
    assert msg.status == "error" and posted == []


class HandoffState(MessagesState):
    trustgraph: str


def handoff_graph(server: Server, first: str, first_tool: str, second: str, second_calls: list[AIMessage]):
    @tool
    def read_benchmarks() -> str:
        """Read public benchmarks."""
        return "median: 100k"

    @tool
    def brief_hr(text: str) -> str:
        """Brief HR."""
        posted.append("hr: " + text)
        return "briefed"

    guard = Guard(
        sources={"read_salaries": "salaries", "read_roadmap": "roadmap", "read_benchmarks": "benchmarks"},
        actions={"post_message": "post_to_engineering", "brief_hr": "brief_hr"},
    )
    tools = [read_salaries, read_roadmap, read_benchmarks, post_message, brief_hr]
    one = create_agent(ScriptedModel(messages=iter([calls((first_tool, {})), AIMessage("notes ready")])),
                       tools, middleware=[guard.middleware()], name="first")
    two = create_agent(ScriptedModel(messages=iter(second_calls)), tools, middleware=[guard.middleware()],
                       name="second")
    graph = StateGraph(HandoffState)
    graph.add_node("first", agent_node(server.client(first), one, first=True))
    graph.add_node("second", agent_node(server.client(second), two))
    graph.add_edge(START, "first")
    graph.add_edge("first", "second")
    graph.add_edge("second", END)
    return graph.compile()


def test_langgraph_handoff_carries_the_session(handoff_server: Server) -> None:
    graph = handoff_graph(handoff_server, "researcher", "read_salaries", "planner", [
        calls(("read_roadmap", {})),
        calls(("post_message", {"text": "Q3 summary"})),
        calls(("brief_hr", {"text": "Q3 summary"})),
        AIMessage("done"),
    ])
    out = graph.invoke({"messages": [HumanMessage("plan Q3")], "trustgraph": ""})
    results = [m for m in out["messages"] if isinstance(m, ToolMessage)]
    denied = next(m for m in results if m.name == "post_message")
    assert denied.status == "error" and "`salaries` is readable by hr only" in denied.content
    assert posted == ["hr: Q3 summary"]
    assert out["trustgraph"].startswith("tg1.")


def test_langgraph_handoff_to_an_uncleared_agent_stops_the_run(handoff_server: Server) -> None:
    graph = handoff_graph(handoff_server, "planner", "read_roadmap", "researcher", [AIMessage("never")])
    with pytest.raises(TrustGraphError) as e:
        graph.invoke({"messages": [HumanMessage("go")], "trustgraph": ""})
    assert e.value.code == "unknown_artifact"


def test_server_down_fails_closed(server: Server) -> None:
    node = ToolNode([as_source(read_roadmap, "roadmap"), as_action(post_message, "post_to_slack")])
    with server.client("planner").session():
        run_tools(node, calls(("read_roadmap", {})))
        server.stop()
        [msg] = run_tools(node, calls(("post_message", {"text": "x"})))
    assert msg.status == "error" and "unreachable" in msg.content and posted == []


def test_wrapped_tools_async(server: Server) -> None:
    @tool
    async def apost(text: str) -> str:
        """Post, asynchronously."""
        posted.append(text)
        return "posted"

    node = ToolNode([as_source(read_salaries, "salaries"), as_action(apost, "post_to_slack"),
                     as_action(post_message, "post_to_slack")])

    async def run() -> list[ToolMessage]:
        async with server.client("planner").session():
            g = graph_of(node)
            await g.ainvoke({"messages": [calls(("read_salaries", {}))]})
            out = await g.ainvoke({"messages": [calls(("apost", {"text": "x"}), ("post_message", {"text": "y"}))]})
            return out["messages"][1:]

    msgs = asyncio.run(run())
    assert [m.status for m in msgs] == ["error", "error"] and posted == []


# -- nested wrapped tools: each is checked ----------------------------------


def nested_tools() -> tuple[Any, Any, Any]:
    post = as_action(post_message, "post_to_slack")

    @tool
    async def apost(text: str) -> str:
        """Post, asynchronously."""
        posted.append(text)
        return "posted"

    return post, as_action(apost, "post_to_slack"), as_source(read_roadmap, "roadmap")


def test_sync_action_inside_an_async_tool_is_checked(server: Server) -> None:
    """The reviewer's lc1.py: an async tool whose body calls a wrapped action synchronously."""
    post, _, _ = nested_tools()

    @tool
    async def digest(topic: str) -> str:
        """Digest."""
        return post.invoke({"text": "salary digest: nina 120k"})

    async def run() -> Any:
        with server.client("planner").session() as s:
            s.read("salaries", "nina: 120k")
            return await as_source(digest, "roadmap").ainvoke({"topic": "x"})

    out = asyncio.run(run())
    assert posted == [] and "TrustGraph blocked `post_to_slack`" in out


def test_sync_action_inside_a_sync_tool_is_checked(server: Server) -> None:
    post, _, _ = nested_tools()

    @tool
    def digest(topic: str) -> str:
        """Digest."""
        return post.invoke({"text": "salary digest"})

    with server.client("planner").session() as s:
        s.read("salaries", "nina: 120k")
        out = as_source(digest, "roadmap").invoke({"topic": "x"})
    assert posted == [] and "TrustGraph blocked `post_to_slack`" in out


def test_async_actions_inside_an_async_tool_are_checked(server: Server) -> None:
    post, apost, _ = nested_tools()

    @tool
    async def digest(topic: str) -> str:
        """Digest."""
        return " | ".join([await apost.ainvoke({"text": "a"}), await post.ainvoke({"text": "b"})])

    async def run() -> Any:
        async with server.client("planner").session() as s:
            await asyncio.to_thread(s.read, "salaries", "nina: 120k")
            return await as_source(digest, "roadmap").ainvoke({"topic": "x"})

    out = asyncio.run(run())
    assert posted == [] and out.count("TrustGraph blocked `post_to_slack`") == 2


def test_a_sync_tool_called_async_is_checked_once(server: Server) -> None:
    post, _, roadmap = nested_tools()
    planner, bodies = recording(server, "planner")
    checks = lambda: sum(1 for b in bodies if "action" in b)  # noqa: E731

    async def run() -> Any:
        async with planner.session():
            await roadmap.ainvoke({})
            return await post.ainvoke({"text": "Q3"})

    assert asyncio.run(run()) == "posted" and posted == ["Q3"] and checks() == 1


# -- agent_node: the hand-off cannot be dropped silently --------------------


def two_nodes(server: Server, state: Any, checkpointer: Any = None) -> Any:
    researcher_tool = as_source(read_salaries, "salaries")
    post = as_action(post_message, "post_to_engineering")

    def research(state: Any, config: Any = None) -> Any:
        researcher_tool.invoke({})
        return {"messages": [AIMessage("notes from salaries")]}

    def plan(state: Any, config: Any = None) -> Any:
        from trustgraph import current_session
        current_session().read("roadmap", "Q3")  # type: ignore[union-attr]
        return {"messages": [AIMessage(post.invoke({"text": "plan"}))]}

    from langchain_core.runnables import RunnableLambda

    g = StateGraph(state)
    g.add_node("research", agent_node(server.client("researcher"), RunnableLambda(research), first=True))
    g.add_node("plan", agent_node(server.client("planner"), RunnableLambda(plan)))
    g.add_edge(START, "research")
    g.add_edge("research", "plan")
    g.add_edge("plan", END)
    return g.compile(checkpointer=checkpointer)


def test_agent_node_refuses_a_state_without_the_key(handoff_server: Server) -> None:
    """The reviewer's lc2.py: MessagesState has no `trustgraph`, so LangGraph dropped the token."""
    with pytest.raises(TrustGraphError) as e:
        two_nodes(handoff_server, MessagesState).invoke({"messages": []})
    assert e.value.code == "handoff_missing" and posted == []


def test_agent_node_requires_a_token_after_the_first(handoff_server: Server) -> None:
    from langchain_core.runnables import RunnableLambda

    g = StateGraph(HandoffState)
    g.add_node("plan", agent_node(handoff_server.client("planner"), RunnableLambda(lambda s: {})))
    g.add_edge(START, "plan")
    g.add_edge("plan", END)
    with pytest.raises(TrustGraphError) as e:
        g.compile().invoke({"messages": [], "trustgraph": ""})
    assert e.value.code == "handoff_missing"
    with pytest.raises(TrustGraphError) as e:
        g.compile().invoke({"messages": [], "trustgraph": "tg1.forged"})
    assert e.value.code == "invalid_handoff"


def test_agent_node_carries_the_token_across_checkpointed_runs(handoff_server: Server) -> None:
    graph = two_nodes(handoff_server, HandoffState, InMemorySaver())
    config = {"configurable": {"thread_id": "t"}}
    out = graph.invoke({"messages": [HumanMessage("plan")], "trustgraph": ""}, config)
    assert "`salaries` is readable by hr only" in out["messages"][-1].content
    # The checkpoint kept the planner's token, so the researcher must now receive the roadmap it is not
    # cleared for: carried, not dropped.
    with pytest.raises(TrustGraphError) as e:
        graph.invoke({"messages": [HumanMessage("again")]}, config)
    assert e.value.code == "unknown_artifact" and posted == []
    with pytest.raises(TrustGraphError) as e:  # resetting the token on a thread with history
        graph.invoke({"messages": [HumanMessage("again")], "trustgraph": ""}, config)
    assert e.value.code == "resumed_without_session" and posted == []


# -- Guard: resumed, checkpointed and evicted threads -----------------------


class GuardState(MessagesState):
    trustgraph: str


def checkpointed(node: ToolNode, state: Any) -> Any:
    g = StateGraph(state)
    g.add_node("tools", node)
    g.add_edge(START, "tools")
    g.add_edge("tools", END)
    return g.compile(checkpointer=InMemorySaver())


def guard_for(server: Server, **kw: Any) -> Guard:
    return Guard(sources={"read_salaries": "salaries", "read_roadmap": "roadmap"},
                 actions={"post_message": "post_to_slack"}, client=server.client("planner"), **kw)


def last(graph: Any, message: AIMessage, thread: str) -> ToolMessage:
    return graph.invoke({"messages": [message]}, {"configurable": {"thread_id": thread}})["messages"][-1]


@pytest.mark.parametrize("state", [GuardState, MessagesState])
def test_guard_thread_resumed_by_a_new_guard(server: Server, state: Any) -> None:
    """A new process (a new Guard) resumes a checkpointed thread that read salaries."""
    first = guard_for(server)
    graph = checkpointed(ToolNode([read_salaries, read_roadmap, post_message], wrap_tool_call=first.wrap_tool_call),
                         state)
    assert last(graph, calls(("read_salaries", {})), "a").content == "nina: 120k, sam: 95k"
    second = guard_for(server)
    graph.builder.nodes["tools"].runnable._wrap_tool_call = second.wrap_tool_call  # type: ignore[attr-defined]
    msg = last(graph, calls(("post_message", {"text": "x"})), "a")
    assert msg.status == "error" and posted == []
    if state is GuardState:  # restored from the token kept in the checkpoint: the real reason
        assert "`salaries`" in msg.content and "denied" in msg.content
    else:  # nowhere to keep the token: refused as unrecorded history
        assert "unrecorded_history" in msg.content


@pytest.mark.parametrize("state", [GuardState, MessagesState])
def test_guard_evicted_thread_is_not_unchecked(server: Server, state: Any) -> None:
    guard = guard_for(server, max_threads=1)
    graph = checkpointed(ToolNode([read_salaries, read_roadmap, post_message], wrap_tool_call=guard.wrap_tool_call),
                         state)
    last(graph, calls(("read_salaries", {})), "a")
    last(graph, calls(("read_roadmap", {})), "b")  # evicts thread a
    msg = last(graph, calls(("post_message", {"text": "x"})), "a")
    assert msg.status == "error" and posted == []
    assert ("`salaries`" in msg.content) if state is GuardState else ("unrecorded_history" in msg.content)


def test_guard_new_session_on_a_resumed_thread_is_refused(server: Server) -> None:
    guard = Guard(sources={"read_salaries": "salaries", "read_roadmap": "roadmap"},
                  actions={"post_message": "post_to_slack"})
    graph = checkpointed(ToolNode([read_salaries, read_roadmap, post_message], wrap_tool_call=guard.wrap_tool_call),
                         MessagesState)
    with server.client("planner").session():
        last(graph, calls(("read_salaries", {})), "a")
    with server.client("planner").session() as s:  # a new session for the same conversation
        s.read("roadmap", "Q3")
        msg = last(graph, calls(("post_message", {"text": "x"})), "a")
    assert msg.status == "error" and "unrecorded_history" in msg.content and posted == []

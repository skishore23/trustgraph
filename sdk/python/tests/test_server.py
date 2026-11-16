"""The SDK against a real `trustgraph serve`."""

from __future__ import annotations

import asyncio
import json
import socket
from typing import Any

import pytest

import trustgraph
from trustgraph import Denied, NeedsApproval, TrustGraphError

from .conftest import Server


def recording(server: Server, agent: str) -> tuple[trustgraph.Client, list[dict[str, Any]]]:
    """A client whose request bodies are kept, sent over the real transport."""
    from trustgraph._client import _urllib_transport

    bodies: list[dict[str, Any]] = []

    def transport(method: str, url: str, headers: dict[str, str], body: bytes | None, timeout: float):
        if body:
            bodies.append(json.loads(body))
        return _urllib_transport(method, url, headers, body, timeout)

    return server.client(agent, transport=transport), bodies


def test_read_then_act_is_denied_with_the_blocking_source(server: Server) -> None:
    planner, bodies = recording(server, "planner")
    with planner.session() as s:
        s.read("salaries", "nina: 120k, sam: 95k")
        s.read("roadmap", "Q3: ship the new billing page")
        with pytest.raises(Denied) as e:
            s.act("post_to_slack", "Q3 headcount plan")
    d = e.value.decision
    assert (d.verdict, d.code) == ("deny", "audience_outside_readers")
    assert [b.source for b in d.blocking] == ["salaries"]
    assert "salaries" in d.reason
    # No content left the process: only hashes.
    assert bodies and all("content" not in b for b in bodies)
    assert "nina" not in json.dumps(bodies)


def test_allow(server: Server) -> None:
    with server.client("planner").session() as s:
        s.read("roadmap", "Q3: ship the new billing page")
        decision = s.act("post_to_slack", "Q3: billing page")
    assert decision.allowed and decision.code == "allowed"


def test_needs_approval_then_operator_approves_then_allow(server: Server) -> None:
    mailer = server.client("mailer")
    sent = []

    @trustgraph.source("price_sheet")
    def price_sheet() -> str:
        return "Widget: $12"

    @trustgraph.action("email_customer", content=lambda a, k: a[0])
    def email(text: str) -> None:
        sent.append(text)

    with mailer.session():
        price_sheet()
        with pytest.raises(NeedsApproval) as e:
            email("Dear Acme, a widget is $12.")
        assert sent == []
        with pytest.raises(NeedsApproval):
            trustgraph.wait_for_approval(e.value.decision, timeout=0.2, poll=0.05)
        server.approve(e.value.decision_id)
        assert trustgraph.wait_for_approval(e.value.decision, timeout=5, poll=0.1).allowed
        email("Dear Acme, a widget is $12.")
    assert sent == ["Dear Acme, a widget is $12."]


def test_handoff_carries_restrictions(handoff_server: Server) -> None:
    researcher = handoff_server.client("researcher")
    planner = handoff_server.client("planner")
    with researcher.session() as r:
        r.read("salaries", "nina: 120k")
        r.output("planning notes", step="notes")
        token = r.handoff()
    with planner.session(carry=token) as p:
        p.read("roadmap", "Q3 roadmap")
        with pytest.raises(Denied) as e:
            p.act("post_to_engineering", "Q3 summary")  # laundering through a second agent
        assert [b.source for b in e.value.decision.blocking] == ["salaries"]
        assert p.act("brief_hr", "Q3 summary").allowed
    # A session without the hand-off can post the roadmap: the restriction is the hand-off's.
    with planner.session() as p:
        p.read("roadmap", "Q3 roadmap")
        assert p.act("post_to_engineering", "Q3 summary").allowed


def test_uncleared_receiver_is_refused(handoff_server: Server) -> None:
    planner = handoff_server.client("planner")
    researcher = handoff_server.client("researcher")
    with planner.session() as p:
        p.read("roadmap", "Q3 roadmap")  # researcher is not listed for roadmap
        token = p.handoff()
    with pytest.raises(TrustGraphError) as e:
        researcher.session(carry=token)
    assert (e.value.status, e.value.code) == (404, "unknown_artifact")
    s = researcher.session()
    with pytest.raises(TrustGraphError):
        s.receive(token)
    with pytest.raises(TrustGraphError) as e:
        s.read("benchmarks", "public")
    assert e.value.code == "session_broken"


def test_server_down_fails_closed(server: Server) -> None:
    ran = []

    @trustgraph.action("post_to_slack")
    def post(text: str) -> None:
        ran.append(text)

    planner = server.client("planner")
    with planner.session() as s:
        s.read("roadmap", "Q3")
        server.stop()
        with pytest.raises(TrustGraphError) as e:
            post("Q3")
    assert e.value.code == "unreachable" and ran == []


def test_unused_port_fails_closed(server: Server) -> None:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    client = trustgraph.Client(f"http://127.0.0.1:{port}", token="x" * 64, timeout=2)
    with pytest.raises(TrustGraphError) as e:
        client.check("a" * 64, "post_to_slack")
    assert e.value.code == "unreachable"


def test_wrong_token_is_refused(server: Server) -> None:
    with pytest.raises(TrustGraphError) as e:
        trustgraph.Client(server.url, token="0" * 64).register_source("roadmap", "Q3")
    assert e.value.status == 401


def test_async_session_propagates_to_tasks(server: Server) -> None:
    @trustgraph.source("salaries")
    async def salaries() -> str:
        await asyncio.sleep(0)
        return "nina: 120k"

    @trustgraph.source("roadmap")
    async def roadmap() -> str:
        return "Q3"

    @trustgraph.action("post_to_slack")
    async def post(text: str) -> str:
        return "posted"

    async def run() -> None:
        async with server.client("planner").session() as s:
            await asyncio.gather(salaries(), roadmap())
            assert len(s.seen) == 2
            with pytest.raises(Denied):
                await post("summary")
        async with server.client("planner").session():
            await roadmap()
            assert await post("summary") == "posted"

    asyncio.run(run())


def test_explain_and_policy(server: Server) -> None:
    planner = server.client("planner")
    with planner.session() as s:
        s.read("salaries", "x")
        notes = s.output("notes")
    assert planner.explain(notes)["sources"] == ["salaries"]
    assert planner.policy()["revision"] == 1
    assert planner.policy_check(planner.policy()["policy"])["ok"] is True


def test_recipients_forms(handoff_server: Server) -> None:
    """A str is one recipient (not its characters); [] is no audience at all and is denied."""
    with handoff_server.client("planner").session() as s:
        s.read("roadmap", "Q3")
        assert s.act("post_to_engineering", "x", "sam").allowed
        assert s.act("post_to_engineering", "x", ["sam"]).allowed
        for recipients in ([], "nina"):
            with pytest.raises(Denied) as e:
                s.act("post_to_engineering", "x", recipients)
            assert e.value.decision.code == "recipients_outside_audience"

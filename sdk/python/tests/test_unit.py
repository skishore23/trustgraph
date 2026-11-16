"""Unit tests against a fake transport: request shapes and fail-closed paths."""

from __future__ import annotations

import asyncio
import contextvars
import hashlib
import json
import threading
from typing import Any

import pytest

import trustgraph
from trustgraph import Client, Denied, NeedsApproval, TrustGraphError

ID = "a" * 64


class Fake:
    """Answers like the server; records every request."""

    def __init__(self, verdict: str = "allow") -> None:
        self.requests: list[tuple[str, str, dict[str, Any] | None, dict[str, str]]] = []
        self.verdict = verdict
        self.fail: Exception | None = None
        self.status: tuple[int, bytes] | None = None
        self.unknown: set[str] = set()  # artifact ids the server does not know (404 on explain)
        self.n = 0

    def __call__(self, method: str, url: str, headers: dict[str, str], body: bytes | None, timeout: float):
        self.requests.append((method, url.split("8787", 1)[1], body and json.loads(body), headers))
        if self.fail:
            raise self.fail
        if self.status:
            return self.status
        path = url.split("8787", 1)[1]
        if path == "/v1/check":
            return 200, json.dumps({
                "decision": "d" * 64, "verdict": self.verdict, "code": self.verdict, "reason": "because",
                "blocking": [{"source": "salaries", "readers": ["hr"], "excluded": ["sam"]}], "expires_at": None,
            }).encode()
        if path.startswith("/v1/artifacts/"):
            if path[-64:] in self.unknown:
                return 404, b'{"error":{"code":"unknown_artifact","message":"no"}}'
            return 200, json.dumps({"artifact": path[-64:], "sources": ["s"], "readers": []}).encode()
        self.n += 1
        return 200, json.dumps({"artifact": f"{self.n:064x}"}).encode()

    def bodies(self, path: str) -> list[dict[str, Any]]:
        return [b for (_, p, b, _) in self.requests if p == path and b is not None]


def client(fake: Fake, **kw: Any) -> Client:
    return Client(token="secret-token", transport=fake, **kw)


def test_token_and_no_content_by_default() -> None:
    fake = Fake()
    c = client(fake)
    c.register_source("salaries", "nina: 120k")
    c.record_output(["b" * 64], "plan", content={"x": 1})
    (method, path, body, headers) = fake.requests[0]
    assert headers["Authorization"] == "Bearer secret-token"
    assert body == {"source": "salaries", "sha256": hashlib.sha256(b"nina: 120k").hexdigest()}
    assert fake.requests[1][2] == {"inputs": ["b" * 64], "step": "plan", "sha256": hashlib.sha256(b'{"x":1}').hexdigest()}
    assert "secret-token" not in repr(c)


def test_send_content_opt_in() -> None:
    fake = Fake()
    client(fake, send_content=True).register_source("roadmap", "Q3")
    assert fake.bodies("/v1/sources") == [{"source": "roadmap", "content": "Q3"}]


def test_token_xor_token_file() -> None:
    with pytest.raises(ValueError):
        Client()
    with pytest.raises(ValueError):
        Client(token="a", token_file="b")


def test_check_returns_every_verdict() -> None:
    d = client(Fake("deny")).check(ID, "post", ["sam"])
    assert (d.verdict, d.allowed, d.blocking[0].source, d.recipients) == ("deny", False, "salaries", ("sam",))


@pytest.mark.parametrize("answer", [
    (200, b"<html>"),
    (200, json.dumps({"verdict": "maybe", "decision": "x", "code": "c", "reason": "r"}).encode()),
    (200, json.dumps({"code": "c"}).encode()),
])
def test_unexpected_check_answer_fails_closed(answer: tuple[int, bytes]) -> None:
    fake = Fake()
    fake.status = answer
    with pytest.raises(TrustGraphError) as e:
        client(fake).check(ID, "post")
    assert e.value.code == "invalid_response"


def test_server_errors_carry_status_and_code() -> None:
    fake = Fake()
    fake.status = (503, b'{"error":{"code":"restart_required","message":"restart"}}')
    with pytest.raises(TrustGraphError) as e:
        client(fake).check(ID, "post")
    assert (e.value.status, e.value.code) == (503, "restart_required")


def test_transport_failure_fails_closed() -> None:
    fake = Fake()
    fake.fail = ConnectionRefusedError("refused")
    with pytest.raises(TrustGraphError) as e:
        client(fake).register_source("s", "x")
    assert e.value.code == "unreachable" and e.value.status is None


def test_session_attaches_everything_seen() -> None:
    fake = Fake()
    with client(fake).session() as s:
        assert s.read("salaries", "nina") == "nina"
        s.read("roadmap", "Q3")
        notes = s.output("notes", step="notes")
        s.act("post", "summary")
    outputs = fake.bodies("/v1/outputs")
    assert outputs[0]["inputs"] == [f"{1:064x}", f"{2:064x}"]
    assert outputs[1]["inputs"] == [f"{1:064x}", f"{2:064x}", notes.id]
    assert outputs[1]["step"] == "post"


def test_act_raises_on_deny_and_needs_approval() -> None:
    for verdict, exc in (("deny", Denied), ("needs_approval", NeedsApproval)):
        with client(Fake(verdict)).session() as s:
            s.read("salaries", "x")
            with pytest.raises(exc) as e:
                s.act("post", "y")
            assert e.value.decision.verdict == verdict


def test_nothing_read_cannot_be_checked() -> None:
    fake = Fake()
    with client(fake).session() as s, pytest.raises(TrustGraphError) as e:
        s.act("post", "y")
    assert e.value.code == "nothing_read"
    assert fake.requests == []


def test_more_than_64_inputs_fold_into_merges() -> None:
    fake = Fake()
    s = client(fake).session()
    for i in range(130):
        s.read("s", str(i))
    s.output("x")
    outputs = fake.bodies("/v1/outputs")
    assert [len(o["inputs"]) for o in outputs] == [64, 64, 2, 3]
    assert all(o["step"] == "trustgraph.merge" for o in outputs[:3])


def test_handoff_round_trip_and_bad_tokens() -> None:
    fake = Fake()
    a = client(fake).session()
    a.read("salaries", "x")
    token = a.handoff()
    b = client(fake).session(carry=token)
    assert b.seen == a.seen
    for bad in ("nope", "tg1.!!!", "tg1." + "e30"):
        with pytest.raises(TrustGraphError) as e:
            client(fake).session(carry=bad)
        assert e.value.code == "invalid_handoff"


def test_failed_receive_breaks_the_session() -> None:
    fake = Fake()
    a = client(fake).session()
    a.read("salaries", "x")
    token = a.handoff()
    fake.status = (404, b'{"error":{"code":"unknown_artifact","message":"no"}}')
    b = client(fake).session()
    with pytest.raises(TrustGraphError):
        b.receive(token)
    fake.status = None
    for attempt in (lambda: b.read("roadmap", "y"), lambda: b.act("post", "z")):
        with pytest.raises(TrustGraphError) as e:
            attempt()
        assert e.value.code == "session_broken"


def test_decorators_need_a_session() -> None:
    ran = []

    @trustgraph.source("salaries")
    def read() -> str:
        ran.append("read")
        return "x"

    @trustgraph.action("post")
    def post(text: str) -> None:
        ran.append("post")

    with pytest.raises(TrustGraphError) as e:
        read()
    assert e.value.code == "no_session"
    with pytest.raises(TrustGraphError):
        post("hi")
    assert ran == []

    @trustgraph.source("salaries", require_session=False)
    def loose() -> str:
        return "x"

    assert loose() == "x"


def test_action_body_runs_only_on_allow() -> None:
    ran = []

    @trustgraph.source("salaries", content=lambda r: r["text"])
    def read() -> dict[str, str]:
        return {"text": "nina: 120k"}

    @trustgraph.action("post", content=lambda a, k: a[0], recipients=lambda a, k: k.get("to"))
    def post(text: str, to: list[str] | None = None) -> str:
        ran.append(text)
        return "sent"

    fake = Fake("deny")
    with client(fake).session():
        read()
        with pytest.raises(Denied):
            post("summary", to=["sam"])
    assert ran == []
    assert fake.bodies("/v1/sources")[0]["sha256"] == hashlib.sha256(b"nina: 120k").hexdigest()
    assert fake.bodies("/v1/check")[0]["recipients"] == ["sam"]

    fake = Fake("allow")
    with client(fake).session():
        read()
        assert post("summary") == "sent"
    assert ran == ["summary"]


def test_async_decorators_and_context_propagation() -> None:
    @trustgraph.source("salaries")
    async def read() -> str:
        return "x"

    @trustgraph.action("post")
    async def post(text: str) -> str:
        return "sent"

    fake = Fake("deny")

    async def main() -> None:
        async with client(fake).session() as s:
            await asyncio.gather(asyncio.create_task(read()))
            with pytest.raises(Denied):
                await post("y")
            assert len(s.seen) == 1

    asyncio.run(main())


def test_session_follows_copied_context_into_threads() -> None:
    fake = Fake()
    seen: list[Any] = []
    with client(fake).session() as s:
        ctx = contextvars.copy_context()
        t = threading.Thread(target=lambda: seen.append(ctx.run(trustgraph.current_session)))
        t.start()
        t.join()
    assert seen == [s]
    assert trustgraph.current_session() is None


def test_wait_for_approval() -> None:
    fake = Fake("needs_approval")
    d = client(fake).check(ID, "email")
    with pytest.raises(NeedsApproval):
        trustgraph.wait_for_approval(d, timeout=0.05, poll=0.01)
    fake.verdict = "allow"
    assert trustgraph.wait_for_approval(d, timeout=1, poll=0.01).allowed
    fake.verdict = "deny"
    with pytest.raises(Denied):
        trustgraph.wait_for_approval(d, timeout=1, poll=0.01)


def test_default_client() -> None:
    trustgraph.set_default_client(None)
    with pytest.raises(TrustGraphError):
        trustgraph.session()
    c = client(Fake())
    trustgraph.set_default_client(c)
    try:
        assert trustgraph.session().client is c
    finally:
        trustgraph.set_default_client(None)


# -- the HTTP client: redirects, plain http, the token ----------------------


def test_redirect_is_refused_and_the_token_stays_home() -> None:
    """A redirect would carry the bearer token to whatever host it names."""
    from http.server import BaseHTTPRequestHandler, HTTPServer

    other_host: list[tuple[str, str | None]] = []

    class Other(BaseHTTPRequestHandler):
        def log_message(self, *args: Any) -> None:
            pass

        def do_GET(self) -> None:
            other_host.append((self.path, self.headers.get("Authorization")))
            body = json.dumps({"decision": "d" * 64, "verdict": "allow", "code": "allowed", "reason": "ok",
                               "blocking": [], "expires_at": None}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(body)

        do_POST = do_GET

    class Redirect(BaseHTTPRequestHandler):
        def log_message(self, *args: Any) -> None:
            pass

        def do_POST(self) -> None:
            self.send_response(302)
            self.send_header("Location", f"http://localhost:{other.server_port}/x")
            self.send_header("Content-Length", "0")
            self.end_headers()

        do_GET = do_POST

    other = HTTPServer(("127.0.0.1", 0), Other)
    redirect = HTTPServer(("127.0.0.1", 0), Redirect)
    for srv in (other, redirect):
        threading.Thread(target=srv.serve_forever, daemon=True).start()
    try:
        c = Client(f"http://127.0.0.1:{redirect.server_port}", token="SECRET-TOKEN")
        with pytest.raises(TrustGraphError) as e:
            c.check(ID, "post_to_slack")
        assert (e.value.code, e.value.status) == ("redirect_refused", 302)
        assert other_host == []
        assert "SECRET-TOKEN" not in str(e.value)
    finally:
        other.shutdown()
        redirect.shutdown()


@pytest.mark.parametrize("url", ["http://10.0.0.5:8787", "http://trustgraph.example.com", "http://[2001:db8::1]:80"])
def test_plain_http_to_another_host_is_refused(url: str) -> None:
    with pytest.raises(ValueError, match="allow_insecure"):
        Client(url, token="t")
    assert Client(url, token="t", allow_insecure=True).url == url


@pytest.mark.parametrize("url", ["http://127.0.0.1:8787", "http://localhost:1", "http://[::1]:8787",
                                 "https://trustgraph.example.com"])
def test_loopback_http_and_any_https_are_accepted(url: str) -> None:
    assert Client(url, token="t").url == url


@pytest.mark.parametrize("url", ["ftp://127.0.0.1", "127.0.0.1:8787", "http://user:pw@127.0.0.1:8787"])
def test_other_urls_are_refused(url: str) -> None:
    with pytest.raises(ValueError):
        Client(url, token="t")


def test_the_token_never_appears_in_errors() -> None:
    fake = Fake()
    fake.fail = OSError("proxy said: Authorization: Bearer secret-token")
    with pytest.raises(TrustGraphError) as e:
        client(fake).register_source("s", "x")
    assert "secret-token" not in str(e.value) and "[token]" in str(e.value)
    fake.fail = None
    fake.status = (401, b'{"error":{"code":"unauthorized","message":"bad token secret-token"}}')
    with pytest.raises(TrustGraphError) as e:
        client(fake).register_source("s", "x")
    assert "secret-token" not in str(e.value)


# -- content that cannot be hashed ------------------------------------------


@pytest.mark.parametrize("value", [None, object(), (i for i in range(3)), {1, 2}, float("nan")])
def test_unhashable_content_raises_invalid_content(value: Any) -> None:
    fake = Fake()
    s = client(fake).session()
    with pytest.raises(trustgraph.InvalidContent) as e:
        s.read("salaries", value)
    assert e.value.code == "invalid_content" and isinstance(e.value, TrustGraphError)
    s.read("roadmap", "Q3")
    with pytest.raises(trustgraph.InvalidContent):
        s.act("post", value)
    assert fake.bodies("/v1/check") == []


def test_source_on_a_generator_needs_content() -> None:
    with pytest.raises(TypeError, match="content="):
        @trustgraph.source("salaries")
        def rows():  # type: ignore[no-untyped-def]
            yield "nina: 120k"

    @trustgraph.source("salaries")
    def returns_a_generator():  # type: ignore[no-untyped-def]
        return (r for r in ["nina: 120k"])

    @trustgraph.source("salaries", content=lambda r: r["text"])
    def picked() -> dict[str, Any]:
        return {"text": "nina: 120k", "handle": object()}

    with client(Fake()).session():
        with pytest.raises(trustgraph.InvalidContent):
            returns_a_generator()
        assert picked()["text"] == "nina: 120k"


def test_a_str_recipient_is_one_name() -> None:
    fake = Fake()

    @trustgraph.action("post", recipients=lambda a, k: k["to"])
    def post(text: str, to: str) -> None:
        pass

    with client(fake).session() as s:
        s.read("roadmap", "Q3")
        s.act("post", "x", "sam")
        post("y", to="ravi")
    assert [b["recipients"] for b in fake.bodies("/v1/check")] == [["sam"], ["ravi"]]


@pytest.mark.parametrize("url", ["http://127.0.0.1:8787/?x=1", "http://127.0.0.1:8787#@evil.com", "https://h/a?"])
def test_urls_with_a_query_or_fragment_are_refused(url: str) -> None:
    with pytest.raises(ValueError):
        Client(url, token="t")


# -- persisted tokens never shed ---------------------------------------------


def test_broken_sessions_persist_a_marker_and_never_hand_off() -> None:
    fake = Fake()
    s = client(fake).session()
    s.read("salaries", "x")
    fake.unknown.add("f" * 64)
    with pytest.raises(TrustGraphError):
        s.receive(trustgraph._session._token(["f" * 64]))
    with pytest.raises(TrustGraphError) as e:
        s.handoff()
    assert e.value.code == "session_broken"
    stored = s.persist()
    ids, broken = trustgraph._session._parse(stored)
    assert set(ids) == {f"{1:064x}", "f" * 64} and broken
    restored = client(Fake()).session()
    with pytest.raises(TrustGraphError) as e:
        restored.receive(stored)
    assert e.value.code == "session_broken" and restored.broken


def test_merge_tokens_is_a_union_and_broken_is_sticky() -> None:
    from trustgraph._session import _parse, _token

    a, b = _token(["a" * 64]), _token(["b" * 64], "lost a read")
    assert trustgraph.merge_tokens("", None) == ""
    assert _parse(trustgraph.merge_tokens(a, "")) == (["a" * 64], None)
    assert _parse(trustgraph.merge_tokens(a, b)) == (["a" * 64, "b" * 64], "lost a read")
    assert _parse(trustgraph.merge_tokens(b, _token([])))[1] == "lost a read"
    assert _parse(trustgraph.merge_tokens(a, "garbage"))[1]  # unreadable counts as broken


@pytest.mark.parametrize("seed", range(40))
def test_no_sequence_of_operations_sheds_a_read(seed: int) -> None:
    """Over random sequences of (read, failed receive, save, load), as the integrations
    use them, a session that is not broken carries every artifact ever read, and once a
    broken session was saved every session loaded later is broken."""
    import random

    from trustgraph._session import _token

    rng = random.Random(seed)
    fake = Fake()
    c = client(fake)
    s, stored, ever, saved_broken = c.session(), None, set(), False
    for step in range(30):
        op = rng.choice(["read", "fail_receive", "save", "load"])
        if op == "read":
            try:
                s.read("salaries", f"row {step}")
                ever.add(s.seen[-1])
            except TrustGraphError:
                assert s.broken
            stored = s.persist(stored)  # the integrations save after every read
        elif op == "fail_receive":
            bad = f"{10_000 + step:064x}"
            fake.unknown.add(bad)
            with pytest.raises(TrustGraphError):
                s.receive(_token([bad]))
        elif op == "save":
            stored = s.persist(stored)
        else:
            s = c.session()
            if stored:
                try:
                    s.receive(stored)
                except TrustGraphError:
                    pass
        saved_broken = saved_broken or bool(stored and trustgraph._session._parse(stored)[1])
        if s.broken is None:
            assert ever <= set(s.seen)
            if op == "load":
                assert not saved_broken
        if op == "load" and saved_broken:
            assert s.broken

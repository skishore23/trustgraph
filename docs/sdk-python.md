# Python SDK

`trustgraph` is the Python client for [`trustgraph serve`](serve.md). Python
3.10 or newer, standard library only. The framework integrations built on it
are in [integrations](integrations.md).

```sh
pip install ./sdk/python
```

## Client

```python
client = trustgraph.Client(url="http://127.0.0.1:8787", token_file="~/.trustgraph/serve/tokens/planner.token")
```

One client is one agent. The server takes the agent's identity from the token
alone; nothing in the SDK can name a different agent. Pass `token=` or
`token_file=`, not both.

The token is a bearer credential, so the client guards where it goes. `url`
must be https, or plain http to a loopback address (`127.0.0.0/8`, `::1`,
`localhost`); plain http to any other host raises `ValueError` unless you pass
`allow_insecure=True`. A URL carrying credentials, a query or a fragment is
refused. Redirects are
never followed: a 3xx answer raises `TrustGraphError("redirect_refused")`
and nothing is sent to the address it names. Proxies are not used, and the
token never appears in an exception or a `repr`.

| Method | Route |
| --- | --- |
| `register_source(source, content=None, sha256=None) -> Artifact` | `POST /v1/sources` |
| `record_output(inputs, step, content=None, sha256=None) -> Artifact` | `POST /v1/outputs` |
| `check(artifact, action, recipients=None) -> Decision` | `POST /v1/check` |
| `explain(artifact)`, `policy()`, `propose(yaml)` | `GET /v1/artifacts/{id}`, `GET /v1/policy`, `POST /v1/policy/propose` |
| `policy_check(yaml)`, `policy_test(yaml)`, `policy_explain(yaml)`, `policy_diff(new, old=None)` | `POST /v1/policy/...` |

`check` returns every verdict as a `Decision` (`verdict`, `code`, `reason`,
`blocking`, `expires_at`, `id`, `allowed`); `decision.raise_unless_allowed()`
turns deny and needs-approval into exceptions.

**Content is hashed in the process.** `content` (text, bytes, or a JSON value
as compact JSON with sorted keys) is hashed with SHA-256 and only the hash is
sent. `None`, and anything that is not text, bytes or JSON (a generator, an
arbitrary object), raises `InvalidContent` before anything is sent; nothing
is hashed through `repr()`. `Client(send_content=True)` sends text instead;
the server hashes it the same way and does not store it.

`trustgraph.set_default_client(client)` sets the client that
`trustgraph.session()` uses.

## Sessions

```python
with client.session() as s:
    s.read("salaries", text)                       # registers text as `salaries`; returns it
    notes = s.output(summary, step="notes")        # derived from everything read so far
    s.act("post_to_slack", message)                # recorded and checked; raises unless allowed
```

A session remembers what was read (`s.seen`). `output` records content as
derived from all of it and adds the new artifact to what was seen. `act`
records the content the same way, checks it and returns the allowing
`Decision`, or raises.

`with` makes the session current in this context (a `contextvars` variable),
for the decorators and integrations; `async with` works too. It follows
`asyncio` tasks, and threads started with `contextvars.copy_context().run`.

**Sessions over-restrict by design.** Everything read in a session attaches
to everything written after it, whether or not the output used it. To keep
an unrelated output free of a sensitive source, start a new session per task,
or do the sensitive work in a separate session (a sub-agent) that hands back
only what it may.

Two server limits shape this. An output needs at least one input, so `act`
or `output` in a session that has read nothing raises `nothing_read`
(register the source the content came from). An output takes at most 64
inputs; a session that has seen more folds them into intermediate outputs
(step `trustgraph.merge`), which carry the same restrictions.

**A TrustGraph session must live as long as the conversation it protects;
resuming a conversation with a new session drops its restrictions.** The
earlier turns are back in front of the model, but the new session has not
seen them. Where the framework keeps a conversation, the integrations keep
the session's token with it, and refuse when they can tell a conversation
was resumed without it:

| Integration | Keeps the token in | Refuses when |
| --- | --- | --- |
| LangGraph `agent_node` | graph state, `state["trustgraph"]` (saved with the thread by a checkpointer) | the key is not declared `Annotated[str, merge_tokens]`, or is empty on any node but the `first=True` one, or empty on a thread that already holds model or tool messages (the node raises before its agent runs) |
| LangChain / LangGraph `Guard` | graph state, `state["trustgraph"]`, for its per-thread sessions (`client=`) | per-thread: the key is not declared `Annotated[str, merge_tokens]`; always: the conversation holds a mapped source's result that the session did not record and the session carries no token (mapped tools are refused) |
| OpenAI Agents `Guard.run` | a dict run context, `context["trustgraph"]` (a `RunState` serialises it) | the run continues a conversation (`RunState`, a non-empty `session=`, `previous_response_id`, `conversation_id`, or earlier model or tool items in the input) and the context holds no token |
| Claude Agent SDK `Guard` | a file per SDK session id under `Guard(store=DIR)` (off by default) | at every mapped action and subagent start: the transcript holds a mapped source's result the guard never recorded, or cannot be read and the guard has not seen the session from its start |
| `as_source` / `as_action` (LangChain, OpenAI Agents), `@source` / `@action`, and `Runner` called inside your own `with client.session():` | nowhere: the session is yours | never: there is no resume detection, so the session must live as long as the conversation |

Each detection sees only what the framework shows it: history that was
trimmed or summarised away is not detected.

**A kept token never shrinks.** It is written with `s.persist(stored)`: the
union of what was stored and everything the session saw, never less. A
broken session writes a broken marker into it instead of an empty token,
so every session later restored from it (`receive`) is refused too.
`trustgraph.merge_tokens(a, b)` is the same union, and is the reducer
LangGraph state uses for the key.

## Hand-offs

```python
token = researcher_session.handoff()               # opaque string: the artifact ids it carries
with planner_client.session(carry=token) as p:     # or p.receive(token)
    ...
```

The receiving session starts carrying everything the sender had seen, so a
restriction follows the work to the next agent. The receiving agent must be
listed in `agents` for every source the token carries; if it is not, the
server answers 404 `unknown_artifact`, `receive` raises, and the session
refuses every later read, output, check and hand-off (`session_broken`).
`s.mark_broken(reason)` does the same on purpose, and `s.broken` says why;
`handoff()` on a broken session raises. The token is not signed: code that
drops or edits it drops the restrictions it carries.

## Decorators

```python
@trustgraph.source("salaries")                     # content=lambda result: ... picks what to hash
def read_salaries(): ...

@trustgraph.action("post_to_slack", content=lambda args, kwargs: args[0], recipients=["sam"])
def post_to_slack(text): ...
```

`@source` registers the return value as the source in the current session;
a value that cannot be hashed raises `InvalidContent` instead of being
returned, and a generator function needs `content=`. So does a tool
returning something else that is not JSON, such as a pydantic model
(`content=lambda r: r.model_dump()`) or a LangChain `content_and_artifact`
tuple (`content=lambda r: r[0]`), which is otherwise withheld. `@action` records and
checks before the body runs, and runs it only on allow. `recipients` is a
list or a function of `(args, kwargs)`; a `str` is one name, and `None` means
the action's whole audience. Both work on `async def` functions. With no current
session, both raise `TrustGraphError("no_session")` before the function
runs; `@source(..., require_session=False)` runs untracked instead. `@action`
never runs unchecked.

## Exceptions and failing closed

| Exception | Means |
| --- | --- |
| `Denied` | The policy denied it. `e.decision` has the reason and the `blocking` sources. |
| `NeedsApproval` | A reviewer must approve first. `e.decision_id` is what the operator approves. |
| `InvalidContent` | The content cannot be hashed (`None`, or not text, bytes or JSON). Nothing was sent. |
| `TrustGraphError` | The check could not be completed. `status` (HTTP, or `None`) and `code`. |

`Denied`, `NeedsApproval` and `InvalidContent` are subclasses of
`TrustGraphError`, so `except TrustGraphError` catches every case in which
the action must not run. A server that cannot be reached (`unreachable`), a
redirect (`redirect_refused`), any other non-2xx answer (including 503
`restart_required` and 429 `rate_limited`), an answer that is not a valid
check (`invalid_response`) and a broken session (`session_broken`) all
raise; a wrapped action never runs when its check did not complete.

The [integrations](integrations.md) turn each of these into a tool result
the agent can read, and the tool does not run: "TrustGraph blocked
`<action>`: denied: …" for a deny, "TrustGraph held `<action>`: needs
approval: … (decision <id>)" for needs approval, and "TrustGraph could not
complete the check for `<action>` (<code>: …)" for everything else. A source
result that cannot be registered is withheld from the agent: "TrustGraph
could not register the result as source `<source>` (…). The result was
withheld."

## Waiting for approval

```python
try:
    send_quote(text)
except trustgraph.NeedsApproval as e:
    trustgraph.wait_for_approval(e.decision, timeout=600, poll=2)
    send_quote(text)                               # its own check now allows
```

`wait_for_approval` re-checks until the decision allows (and returns it),
turns to deny (raises `Denied`) or `timeout` passes (raises `NeedsApproval`).
The operator approves on the operator page; see [serve](serve.md).

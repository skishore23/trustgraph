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

**Content is hashed in the process.** `content` (text, bytes, or anything
else as compact JSON with sorted keys) is hashed with SHA-256 and only the
hash is sent. `Client(send_content=True)` sends text instead; the server
hashes it the same way and does not store it.

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
refuses every later read, output and check (`session_broken`). The token is
not signed: code that drops or edits it drops the restrictions it carries.

## Decorators

```python
@trustgraph.source("salaries")                     # content=lambda result: ... picks what to hash
def read_salaries(): ...

@trustgraph.action("post_to_slack", content=lambda args, kwargs: args[0], recipients=["sam"])
def post_to_slack(text): ...
```

`@source` registers the return value as the source in the current session.
`@action` records and checks before the body runs, and runs it only on allow.
`recipients` is a list or a function of `(args, kwargs)`; `None` means the
action's whole audience. Both work on `async def` functions. With no current
session, both raise `TrustGraphError("no_session")` before the function
runs; `@source(..., require_session=False)` runs untracked instead. `@action`
never runs unchecked.

## Exceptions and failing closed

| Exception | Means |
| --- | --- |
| `Denied` | The policy denied it. `e.decision` has the reason and the `blocking` sources. |
| `NeedsApproval` | A reviewer must approve first. `e.decision_id` is what the operator approves. |
| `TrustGraphError` | The check could not be completed. `status` (HTTP, or `None`) and `code`. |

`Denied` and `NeedsApproval` are subclasses of `TrustGraphError`, so
`except TrustGraphError` catches every case in which the action must not run.
A server that cannot be reached (`unreachable`), any non-2xx answer
(including 503 `restart_required` and 429 `rate_limited`) and an answer that
is not a valid check (`invalid_response`) all raise; a wrapped action never
runs when its check did not complete.

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

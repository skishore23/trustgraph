# trustgraph serve

`trustgraph serve` puts the TrustGraph runtime behind a local HTTP API and an
MCP endpoint, so any agent stack can use it. Your system tells it what each
agent read and wrote, and asks right before the agent acts. The answer is
**allow**, **needs approval** or **deny**, from your policy file. A person,
the operator, approves requests and activates policy changes on a plain web
page.

It listens on 127.0.0.1 only. Each agent has its own token. The operator signs
in through a single-use file. Over HTTP and MCP, a request is acted on as the
agent whose token it carries; no agent route or tool can approve, revoke or
activate; and a changed policy file waits for the operator. These hold as
long as agents cannot read the state directory; see
[Threat model](#threat-model).

## Five-minute quickstart

Install the CLI (see the [README](../README.md#quickstart)), then write the
starter policy and start the server. `--operator` must be a person in one of
the policy's groups; here it is `leo`, the sales reviewer.

```sh
trustgraph policy init
trustgraph serve --policy trustgraph.policy.yaml --operator leo --state ~/tg-quickstart
```

```text
TrustGraph serve: http://127.0.0.1:8787
MCP endpoint: http://127.0.0.1:8787/mcp
Operator sign-in (single use): /Users/you/tg-quickstart/operator.html
Agent tokens: /Users/you/tg-quickstart/tokens
Policy in force: revision 1 (from the policy file), digest 8b12ebf75cc8
Operator: leo
```

Without `--state` the state directory is `~/.trustgraph/serve`. The tokens
directory has one file per agent in the policy: `planner.token` and
`mailer.token`.

In another terminal, act as the planner. It reads the roadmap, writes a
summary from it, and asks before posting to Slack:

```sh
T=~/tg-quickstart/tokens
P="Authorization: Bearer $(cat $T/planner.token)"
J="Content-Type: application/json"

curl -s localhost:8787/v1/sources -H "$P" -H "$J" \
  -d '{"source": "roadmap", "content": "Q3: ship the new billing page"}'
```

```text
{"artifact":"59215d71a020cdf80bc36967f18c80a2c9f4d4b07063a4709c1bd847f7436a9e"}
```

```sh
curl -s localhost:8787/v1/outputs -H "$P" -H "$J" \
  -d '{"inputs": ["59215d71…"], "step": "roadmap_summary", "content": "Q3: billing page"}'
curl -s localhost:8787/v1/check -H "$P" -H "$J" \
  -d '{"artifact": "9bdd2fdf…", "action": "post_to_slack"}'
```

```text
{"artifact":"9bdd2fdffce67229a915eb778cf4ea68af676f69a4563e892170a4bd7204af7a"}
{"blocking":[],"code":"allowed","decision":"4817ba11…","expires_at":null,"reason":"allowed","verdict":"allow"}
```

(Artifact and decision ids are 64 hex characters; they are shortened here.
`content` is hashed by the server and not stored; you can send `sha256`
instead.)

Now a plan made from the salaries **and** the roadmap. The check denies it and
names the source that blocks it:

```sh
curl -s localhost:8787/v1/sources -H "$P" -H "$J" \
  -d '{"source": "salaries", "content": "nina: 120k, sam: 95k"}'
curl -s localhost:8787/v1/outputs -H "$P" -H "$J" \
  -d '{"inputs": ["90aab033…", "59215d71…"], "step": "headcount_plan", "content": "Q3 headcount plan"}'
curl -s localhost:8787/v1/check -H "$P" -H "$J" \
  -d '{"artifact": "9722e42d…", "action": "post_to_slack"}'
```

```json
{
  "blocking": [
    { "excluded": ["ravi", "sam"], "readers": ["hr"], "source": "salaries" }
  ],
  "code": "audience_outside_readers",
  "decision": "94da573b…",
  "expires_at": null,
  "reason": "denied: `salaries` is readable by hr only; engineering (ravi, sam) is not included",
  "verdict": "deny"
}
```

The mailer has its own token and may not touch the salaries. To the mailer,
the planner's salaries artifact does not exist:

```sh
M="Authorization: Bearer $(cat $T/mailer.token)"
curl -s localhost:8787/v1/sources -H "$M" -H "$J" -d '{"source": "salaries", "content": "nina: 120k"}'
curl -s localhost:8787/v1/artifacts/90aab033… -H "$M"
```

```text
{"error":{"code":"agent_cannot_use_source","message":"agent `mailer` may not use this source"}}   (HTTP 403)
{"error":{"code":"unknown_artifact","message":"no artifact with that id that this agent may use"}}   (HTTP 404)
```

It drafts a quote from the price sheet. Emailing a customer needs a
reviewer's approval:

```sh
curl -s localhost:8787/v1/sources -H "$M" -H "$J" -d '{"source": "price_sheet", "content": "Widget: $12"}'
curl -s localhost:8787/v1/outputs -H "$M" -H "$J" \
  -d '{"inputs": ["4fd83368…"], "step": "draft_quote", "content": "Dear Acme, a widget is $12."}'
curl -s localhost:8787/v1/check -H "$M" -H "$J" \
  -d '{"artifact": "852d2938…", "action": "email_customer"}'
```

```text
{"blocking":[],"code":"needs_approval","decision":"c191bc63…","expires_at":null,"reason":"needs approval: action `email_customer` requires a reviewer to approve it","verdict":"needs_approval"}
```

Open the operator launch file in your browser
(`open ~/tg-quickstart/operator.html` on macOS). It signs you in once; the
server then writes a new launch file for the next sign-in. The page shows:

```text
Waiting for approval
mailer wants email_customer → acme
derived from: price_sheet
needs approval: action `email_customer` requires a reviewer to approve it
asked 2026-11-15 19:08 · decision c191bc63cff2
[Approve]
```

Click **Approve**. The page repeats exactly what you grant and asks again:

```text
Approve: mailer may email_customer to acme, from price_sheet (decision c191bc63cff2…).
[Approve now] [Cancel]
```

The same works from a terminal. Exchange the launch file's secret for a
session, then send it as `X-TrustGraph-Operator`:

```sh
SECRET=$(sed -n 's/.*#\/connect\/\([0-9a-f]\{64\}\).*/\1/p' ~/tg-quickstart/operator.html)
curl -s -X POST localhost:8787/session -H "Authorization: Bearer $SECRET"
```

```text
{"expires_at":1794827297,"session":"09e2ff22…"}
```

```sh
O="X-TrustGraph-Operator: 09e2ff22…"
curl -s localhost:8787/v1/operator/state -H "$O"
curl -s localhost:8787/v1/operator/approve -H "$O" -H "$J" -d '{"decision": "c191bc63…"}'
```

```text
{"approvals":[],"clock_behind":false,"now":1794798496,"operator":"leo","pending":[{"action":"email_customer","agent":"mailer","can_approve":true,"cannot_approve":null,"decision":"c191bc63…","reason":"needs approval: action `email_customer` requires a reviewer to approve it","recipients":["acme"],"requested_at":1794798496,"reviewers":["leo"],"sources":["price_sheet"]}],"policy":{"digest":"8b12ebf7…","revision":1,"source":"file"},"proposals":[]}
{"approved":true,"decision":"c191bc63…","expires_at":1794800305}
```

The same check now allows, until the approval expires:

```text
{"blocking":[],"code":"allowed","decision":"c191bc63…","expires_at":1794800305,"reason":"allowed: approved by `leo`","verdict":"allow"}
```

The launch secret is now used up. Using it again fails, and the server logs
where the next sign-in link is:

```text
{"error":{"code":"invalid_launch_secret","message":"this sign-in link is not valid or was already used. If you did not use it, stop the server: someone else holds the operator session"}}   (HTTP 401)
```

The whole flow is also in [examples/python_client.py](../examples/python_client.py),
a client with no dependencies. Run it against the same server; with `--wait` it
waits for you to approve:

```sh
python3 examples/python_client.py --tokens ~/tg-quickstart/tokens --wait
```

```text
planner post_to_slack (salaries + roadmap): deny (audience_outside_readers)
  denied: `salaries` is readable by hr only; engineering (ravi, sam) is not included
planner post_to_slack (roadmap only): allow (allowed)
  allowed
mailer email_customer: needs_approval (needs_approval)
  needs approval: action `email_customer` requires a reviewer to approve it
mailer reads salaries: refused (403 agent_cannot_use_source)
Waiting for the operator to approve on the operator page ...
mailer email_customer: allow (allowed)
  allowed: approved by `leo`
Sending the quote now: the check said allow.
```

This output was captured from a real run; only the home directory paths and
the 64-character ids and secrets were shortened.

## The API

JSON in, JSON out. Every response has `Cache-Control: no-store`. Request
bodies are at most 512 KiB and must be `Content-Type: application/json`.
Unknown fields are refused, so no body can name an agent.

Agent routes take `Authorization: Bearer <the agent's token>`:

| Route | Body | Answer |
| --- | --- | --- |
| `POST /v1/sources` | `{"source", "sha256"}` or `{"source", "content"}` | `{"artifact"}`. The agent must be listed in the source's `agents`. |
| `POST /v1/outputs` | `{"inputs": [ids], "step", "sha256" \| "content"}` | `{"artifact"}` |
| `POST /v1/check` | `{"artifact", "action", "recipients": [names] \| null}` | `{"decision", "verdict", "code", "reason", "blocking", "expires_at"}`, always 200 |
| `GET /v1/artifacts/{id}` | | `{"artifact", "sources", "readers"}`. Only if the agent may use every source. |

An artifact the agent may not use (it derives from a source the agent is not
listed for) gets exactly the same answer as an id that does not exist: 404
`unknown_artifact` with the same body, on `GET /v1/artifacts/{id}`,
`/v1/check`, the `inputs` of `/v1/outputs` and the matching MCP tools. Ids
are derived from the source name and the content hash, so different answers
would let an agent confirm a guess about content it may not read. Neither
answer is journaled.

`GET /v1/policy` shows every agent the whole active policy, groups and people
included. This is deliberate: an agent needs the policy to propose a change
to it. Do not put anything in the policy file that agents may not see.
| `POST /v1/policy/propose` | `{"policy": "<yaml>"}` | `{"proposal", "widens", "changes", "explanation"}`, or 422 with `diagnostics`. Never changes the active policy. |
| `GET /v1/policy` | | `{"policy", "revision", "digest"}`. The YAML is the canonical form; comments are not kept. |
| `POST /v1/policy/check`, `/test`, `/explain` | `{"policy"}` | The same JSON as `trustgraph policy check/test/explain --json` |
| `POST /v1/policy/diff` | `{"old"?, "new"}` | As `policy diff --json`; `old` defaults to the active policy |
| `POST /v1/policy/simulate` | `{"policy", "seed"?, "count"? ≤ 8, "steps"? ≤ 120}` | As `policy simulate --json` |
| `GET /v1/policy/schema` | | The policy file's JSON Schema |

Operator routes take only the `X-TrustGraph-Operator: <session>` header, and
refuse any request that also carries an `Authorization` header:

| Route | Body | Does |
| --- | --- | --- |
| `GET /` | | The operator page: a static shell with no data; it loads the rest with the session |
| `POST /session` | `Authorization: Bearer <launch secret>` | Exchanges the single-use secret for `{"session", "expires_at"}`, ends any earlier session and writes a new launch file |
| `GET /v1/operator/state` | | Operator, policy in force, pending decisions, live approvals, proposals |
| `POST /v1/operator/approve` | `{"decision"}` | Approves as `--operator` |
| `POST /v1/operator/revoke` | `{"decision"}` | Revokes a live approval |
| `POST /v1/operator/activate` | `{"proposal"}` | Activates a stored proposal |
| `POST /v1/operator/propose` | `{"policy"}` | Stores a proposal from the operator |

Errors are `{"error": {"code", "message"}}`:

| Status | When |
| --- | --- |
| 400 `invalid_request` | The body is not valid JSON for the route, or has an unknown field |
| 401 | No credential, or a wrong one, for the route; `operator_session_replaced` when a newer sign-in ended this session |
| 403 | A wrong `Host` or `Origin`; `agent_cannot_use_source` (registering a source); `reviewer_not_allowed` |
| 404 | `unknown_artifact` (no such artifact, or not one this agent may use), `unknown_decision`, `unknown_proposal`, or no such route |
| 409 `clock_behind` | An approval while the system clock is behind the journal; see [Time](#time) |
| 413 `payload_too_large` | The body is over 512 KiB |
| 415 | The body is not `application/json` |
| 422 | Any other refusal by the runtime, with its code (for example `invalid_digest`, `unknown_source`, `invalid_policy`) |
| 429 `rate_limited` | Over the agent's rate limit; see `Retry-After` |
| 503 `restart_required` | The journal failed, or a request failed while holding the runtime; restart the server |
| 507 `quota_exceeded` | A server or per-agent artifact or journal cap is reached |

## MCP

The same port serves MCP over streamable HTTP at `/mcp`. Authenticate with the
agent's token in the `Authorization` header. A generic client configuration:

```json
{
  "mcpServers": {
    "trustgraph": {
      "type": "http",
      "url": "http://127.0.0.1:8787/mcp",
      "headers": { "Authorization": "Bearer <contents of tokens/mailer.token>" }
    }
  }
}
```

Give each agent its own entry with its own token. The tools are
`register_source`, `record_output`, `check`, `explain_artifact`, `policy_get`,
`policy_schema`, `policy_check`, `policy_test`, `policy_explain`,
`policy_diff`, `policy_simulate` and `policy_propose`. They return the same
JSON as the HTTP API, with errors marked `isError`. No tool can approve,
revoke or activate. The endpoint is stateless: every request is authenticated
on its own. `register_source`, `record_output`, `check` and `policy_propose`
are annotated as neither read-only nor idempotent, because each call is
journaled.

## Tokens and the operator

- **Agent tokens.** At start, the server writes `<state>/tokens/<agent>.token`
  for every agent in the active policy: 32 random bytes as hex, mode 0600. A
  token is kept across restarts. When an activation adds an agent, its file
  appears; when it removes one, the file is deleted and the old token stops
  working at once. Give each agent only its own file.
- **The operator.** `--operator` names the person every approval, revocation
  and activation is made as. It must be a person of the ACTIVE policy (which
  may differ from the file). Approving still needs that person to be a listed
  reviewer of the action and a reader of the output; the page hides Approve
  and says why when that is not so.
- **Signing in.** `<state>/operator.html` (mode 0600) holds a single-use
  secret. Opening it exchanges the secret for a session id that the page
  keeps in `sessionStorage` (this origin and port only) and sends as
  `X-TrustGraph-Operator`. There is no cookie: cookies ignore ports, so any
  other server on 127.0.0.1 would receive one. A session lasts 8 hours.
- **One session at a time; sign in again without a restart.** Each sign-in
  writes a new single-use `operator.html` and logs that it did. Opening it
  starts a new session and ends the previous one; the old page says "Signed
  out: a newer operator session was started. If that was not you, stop the
  server." If the launch file says it was already used and you did not use
  it, stop the server: someone else holds the operator session.
- **Every request** must be sent to `127.0.0.1:<port>` or `localhost:<port>`,
  and a browser `Origin`, when sent, must be that same origin. Anything else
  is 403.

## Limits and quotas

Agents are untrusted callers.

- **Rate limit:** 50 requests per second per agent, bursts up to 200. The
  pure policy helpers (`check`, `test`, `explain`, `diff`, `schema`) also
  count against 10 per second per agent, and `simulate` against 2 per second.
  Over any of them, 429 with `Retry-After` (an `isError` result over MCP).
- **Server caps:** at most 100,000 artifacts (`--max-artifacts`) and a 1 GiB
  journal (`--max-journal-bytes`).
- **Per-agent budgets,** counted from when the server started: 64 MiB of
  journal (`--agent-journal-bytes`) and 10,000 artifacts (`--agent-artifacts`).
  One agent at its budget gets 507 `quota_exceeded`; other agents are not
  affected.
- Writes past any cap are refused with 507 before anything is written.
  Re-registering content that is already registered still works at a cap, and
  so does `check` at the artifact cap. Operator actions are never blocked.
- **Not journaled:** requests refused before the engine (bad shape, unknown
  source, an artifact the agent may not use) and idempotent re-registrations.
- **Engine state per agent:** at most 64 remembered pending decisions per
  agent (1,024 in all) and 8 stored proposals per agent (32 for all agents;
  the operator and the policy file have their own 32). When an agent is over,
  its own oldest entry goes, so it cannot push out anyone else's.
- **Recovering from a quota:** per-agent budgets reset when the server
  restarts. If the server caps are reached, raise them
  (`--max-artifacts`, `--max-journal-bytes`) or start over with a new state
  directory; there is no compaction yet.
- **`policy_simulate`:** `count` up to 8 and `steps` up to 120 per call.

## Restarts and the journal

Every event is written to `<state>/journal.jsonl` before it takes effect, and
the runtime is rebuilt from the journal at start. A restart keeps artifacts,
pending decisions, approvals, stored proposals and tokens.

- **The policy file applies by itself only on the first start** (an empty
  journal). After that, if the file differs from the active policy, it is
  NOT applied: it is stored once as a proposal from the file, and the server
  prints the difference, with every `WIDENS` line, and how to apply it:

  ```text
  The policy file differs from the active policy. It was NOT applied; it is stored as a proposal from the file. Changes it would make:
    WIDENS  source price_sheet: readers gained beta
    WIDENS  action email_customer: audience gained beta
  To apply it, activate it on the operator page, or restart once with --activate-policy-file.
  ```

  The operator page lists it under "Proposed policies" as "From the policy
  file", with its changes, like any other proposal. `--activate-policy-file`
  applies a changed file at start and prints the same difference; use it only
  as the person at the terminal. A file equal to the active policy changes
  nothing.
- **Every activation voids all approvals** and forgets pending decisions.
  Agents must ask again.
- **The journal is hash-chained.** It detects edits, reordering and gaps. It
  does not detect deleted trailing records or a deleted file. Protect the
  state directory with filesystem permissions.
- **Outcome unknown.** If a journal write fails, the server cannot know
  whether the last event took effect. It stops serving: every request gets 503
  `restart_required` and the reason is logged. Restart it; the journal is
  replayed and the agent asks again.
- The journal grows with every call; there is no compaction yet.

## Time

The server takes the larger of the wall clock and the last journaled time
when it starts, and from then on adds monotonic time; later changes to the
wall clock are ignored. So an approval lasts its `valid_for` of real elapsed
time even if the clock is set back. If at start the wall clock is more than
300 seconds behind the journal (for example after a record was written with
a clock far in the future), the server logs a warning and refuses approvals
with 409 `clock_behind` until the wall clock catches up; checks and
everything else keep working, and the operator page says so.

## Threat model

What the server enforces, and what it assumes.

- **Agents hold only their own token.** The server takes an agent's identity
  from the bearer token alone; no request field can name an agent. A token
  file shared by two agents identifies neither.
- **Agents must not be able to read the state directory.** It holds every
  token, the unused operator launch file and the journal. An agent that runs
  as the same OS user with shell access can read them all and act as any
  agent or as the operator. Run agents as a different OS user or in a
  sandbox, and keep the state directory outside anything agents can read. The
  server refuses a state directory inside the Git repository that holds the
  policy file or the current directory, unless you pass
  `--allow-state-in-repository`.
- **The policy file is not a trust root.** Agents working in its repository
  can edit it (the server warns when it is in one). A changed file never
  applies without the operator, or `--activate-policy-file` at the terminal.
  The FIRST start applies the file as written, so check it before then.
- **Agents propose; the operator decides.** No agent route or MCP tool can
  approve, revoke or activate.
- **The operator approves a content hash, not content they have read.** The
  page shows the agent, action, recipients, sources and decision id. What the
  output says is between your system and the agent.
- **The operator session** is a header, one at a time, from a single-use
  file. If someone else signs in, your page tells you; stop the server.
- **TrustGraph decides; your system acts.** The host must call `check` right
  before acting and act only on `allow`. TrustGraph only knows the sources and
  outputs it was told about.
- **The journal** detects edits, reordering and gaps inside the file, but not
  removal of trailing records or of the whole file. Protect the state
  directory with filesystem permissions.
- **Local only.** It binds 127.0.0.1 and refuses other Host names. Do not put
  it behind a proxy that rewrites Host.

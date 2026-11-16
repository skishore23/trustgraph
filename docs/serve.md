# trustgraph serve

`trustgraph serve` puts the TrustGraph runtime behind a local HTTP API and an
MCP endpoint, so any agent stack can use it. Your system tells it what each
agent read and wrote, and asks right before the agent acts. The answer is
**allow**, **needs approval** or **deny**, from your policy file. A person,
the operator, approves requests and activates policy changes on a plain web
page.

It listens on 127.0.0.1 only. Each agent has its own token. The operator signs
in through a single-use file. No agent can act as another agent, approve its
own request or change the active policy.

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

The mailer has its own token and may not touch the salaries:

```sh
M="Authorization: Bearer $(cat $T/mailer.token)"
curl -s localhost:8787/v1/sources -H "$M" -H "$J" -d '{"source": "salaries", "content": "nina: 120k"}'
```

```text
{"error":{"code":"agent_cannot_use_source","message":"agent `mailer` may not use this source"}}   (HTTP 403)
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
(`open ~/tg-quickstart/operator.html` on macOS). It signs you in once and is
deleted. The page shows:

```text
Waiting for approval
mailer wants email_customer → acme
derived from: price_sheet
needs approval: action `email_customer` requires a reviewer to approve it
[Approve]
```

Click **Approve**. The same check now allows, until the approval expires:

```text
{"blocking":[],"code":"allowed","decision":"c191bc63…","expires_at":1794798040,"reason":"allowed: approved by `leo`","verdict":"allow"}
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

This output was captured from a real run; only the home directory paths were
shortened.

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
| `POST /v1/policy/propose` | `{"policy": "<yaml>"}` | `{"proposal", "widens", "changes", "explanation"}`, or 422 with `diagnostics`. Never changes the active policy. |
| `GET /v1/policy` | | `{"policy", "revision", "digest"}`. The YAML is the canonical form; comments are not kept. |
| `POST /v1/policy/check`, `/test`, `/explain` | `{"policy"}` | The same JSON as `trustgraph policy check/test/explain --json` |
| `POST /v1/policy/diff` | `{"old"?, "new"}` | As `policy diff --json`; `old` defaults to the active policy |
| `POST /v1/policy/simulate` | `{"policy", "seed"?, "count"? ≤ 8, "steps"? ≤ 120}` | As `policy simulate --json` |
| `GET /v1/policy/schema` | | The policy file's JSON Schema |

Operator routes take only the session cookie from the launch file:

| Route | Body | Does |
| --- | --- | --- |
| `GET /` | | The operator page |
| `POST /session` | `Authorization: Bearer <launch secret>` | Exchanges the single-use secret for an HttpOnly, SameSite=Strict cookie |
| `GET /v1/operator/state` | | Operator, policy in force, pending decisions, live approvals, proposals |
| `POST /v1/operator/approve` | `{"decision"}` | Approves as `--operator` |
| `POST /v1/operator/revoke` | `{"decision"}` | Revokes a live approval |
| `POST /v1/operator/activate` | `{"proposal"}` | Activates a stored proposal |
| `POST /v1/operator/propose` | `{"policy"}` | Stores a proposal from the operator |

Errors are `{"error": {"code", "message"}}`:

| Status | When |
| --- | --- |
| 400 `invalid_request` | The body is not valid JSON for the route, or has an unknown field |
| 401 | No credential, or a wrong one, for the route |
| 403 | A wrong `Host` or `Origin`; `agent_cannot_use_source`; `reviewer_not_allowed` |
| 404 | `unknown_artifact`, `unknown_decision`, `unknown_proposal`, or no such route |
| 413 `payload_too_large` | The body is over 512 KiB |
| 415 | The body is not `application/json` |
| 422 | Any other refusal by the runtime, with its code (for example `invalid_digest`, `unknown_source`, `invalid_policy`) |
| 429 `rate_limited` | Over the agent's rate limit; see `Retry-After` |
| 503 `restart_required` | The journal failed; restart the server |
| 507 `quota_exceeded` | The artifact or journal cap is reached |

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
on its own.

## Tokens and the operator

- **Agent tokens.** At start, the server writes `<state>/tokens/<agent>.token`
  for every agent in the active policy: 32 random bytes as hex, mode 0600. A
  token is kept across restarts. When an activation adds an agent, its file
  appears; when it removes one, the file is deleted and the old token stops
  working at once. Give each agent only its own file.
- **The operator.** `--operator` names the person every approval, revocation
  and activation is made as. Approving still needs that person to be a listed
  reviewer of the action and a reader of the output. The operator signs in
  through `<state>/operator.html`, which works once per start and is deleted
  when used. The session is a cookie, never a bearer token, and lasts 8
  hours. To sign in again, restart the server.
- **Every request** must be sent to `127.0.0.1:<port>` or `localhost:<port>`,
  and a browser `Origin`, when sent, must be that same origin. Anything else
  is 403.

## Limits and quotas

Agents are untrusted callers.

- **Rate limit:** 50 requests per second per agent, bursts up to 200. Over it,
  429 with `Retry-After`.
- **Artifacts:** at most 100,000 (`--max-artifacts`). **Journal:** at most
  1 GiB (`--max-journal-bytes`). Agent writes past either cap are refused with
  507 before anything is written. Operator actions are never blocked by them.
- **`policy_simulate`:** `count` up to 8 and `steps` up to 120 per call.

## Restarts and the journal

Every event is written to `<state>/journal.jsonl` before it takes effect, and
the runtime is rebuilt from the journal at start. A restart keeps artifacts,
pending decisions, approvals, stored proposals and tokens.

- **The policy file re-applies only if it changed.** Start with the same file
  and an in-service activation stays in force; the page says "activated in
  service". Start with an edited file and the file wins.
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

## Threat model in plain words

- **Agents hold only their own token.** The server takes an agent's identity
  from its token alone. No request field can name another agent.
- **Keep the state directory outside agent workspaces.** It holds every
  token, the operator launch file and the journal. The server refuses to start
  when the state directory is inside the git repository that holds the policy
  file or the current directory, unless you pass `--allow-state-in-repository`.
- **Agents propose; the operator decides.** An agent can propose a policy. It
  cannot approve, revoke or activate, over HTTP or MCP.
- **TrustGraph decides; your system acts.** The host must call `check` right
  before acting and act only on `allow`. TrustGraph only knows the sources and
  outputs it was told about.
- **Local only.** It binds 127.0.0.1 and refuses other Host names. Do not put
  it behind a proxy that rewrites Host.

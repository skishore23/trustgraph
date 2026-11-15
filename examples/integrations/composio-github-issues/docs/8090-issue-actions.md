# 8090 → TrustGraph → GitHub issue

8090's native agent proposes a GitHub issue. TrustGraph requires human approval
of its exact title, body, repository, connected account and tool binding.
Composio creates the approved issue. This path starts no Codex process and
generates no patch.

For the short live grant/revoke presentation and manual 8090 handoff, use the
[issue demo runbook](one-hour-issue-demo.md). The operator controls a durable
gateway publication permission independently of exact-issue review. Revoking
or restoring permission changes the policy context and requires fresh approval.

8090 supplies the Work Order and project skill; its native workspace runs the
[restricted client](../scripts/trustgraph_actions.py). TrustGraph owns the review
UI, publishing credential and durable journal. This example approves a submitted
action; it does not capture the native agent's earlier context or infer source
permissions. Work-order identifiers are caller-supplied references, not verified
8090 assertions.

## Preview

```sh
cp trustgraph-actions.example.json trustgraph-actions.local.json
cargo run -p trustgraph-composio-example --
```

Set `github_repository` in the ignored local config to your demo repository.
Open the single-use private `operator.html` file printed at startup to establish
an HttpOnly operator session (restart the server to sign in again) at [127.0.0.1:8084](http://127.0.0.1:8084). The separate agent
API is on port 8085; it has no approval or execution routes. The owner and agent ports must differ. Optional
`composio_dashboard_url` and `source_url` config fields set the dashboard and
source links shown in the walkthrough.

Set the real project and Work Order IDs in the [example](../8090/github-issue.json), then submit:

```sh
python3 scripts/trustgraph_actions.py \
  --token-file .trustgraph-live/issue-actions-preview/agent-token \
  propose --file 8090/github-issue.json
```

Approve the displayed issue, then select **Record approved preview**. The receipt
is simulated; no GitHub issue is created. To test approval invalidation, approve
a proposal and then revise it before execution. A revision file contains
`revision` (the current revision), `title` and `body`:

```sh
python3 scripts/trustgraph_actions.py \
  --token-file .trustgraph-live/issue-actions-preview/agent-token \
  revise wo-2-github-issue --file revision.json
```

The revision needs fresh approval. The same proposal key with changed content
is rejected; matching retries return the existing proposal. An uncertain
execution outcome never triggers an automatic resend, including after restart.

## Native 8090 integration

Add [TrustGraph issue approval](../8090/trustgraph-issue-skill.md) as a
General skill in the 8090 project. Its native workspace needs this branch checked
out in a connected setup repository. Follow 8090's built-in **Workspace Setup**
skill to configure and reload the environment.

| Setting | Location |
| --- | --- |
| `TRUSTGRAPH_ACTIONS_URL` | Native workspace environment; HTTPS origin of the agent tunnel |
| `TRUSTGRAPH_AGENT_TOKEN` | Native workspace secret; private agent-token capability |
| `COMPOSIO_API_KEY` | Gateway only; never give it to the native agent |

Tunnel only port 8085, with request inspection disabled. Set `agent_public_url`
to the tunnel's HTTPS origin in the gateway config and restart. The agent router
permits that Host and rejects browser Origin headers. The operator UI stays
local; the tunnel does not create a remote operator session.

The agent inspects `list`, confirms repository and mode, submits an issue and
reports its action ID. It can move the Work Order to In Review and revise the
proposal after feedback. Only the human operator approves and executes. Do not
also grant the native agent direct GitHub write credentials or Composio write
tools when this gateway is the enforced publication path.

## Live Composio execution

Use a Composio GitHub connected account with access to the demo repository.
Keep the Composio key in the gateway's `COMPOSIO_API_KEY` environment, or in
an owner-only `composio-key` file in the live state directory. Never put it in a
prompt, work order, skill or committed file.

```sh
mkdir -m 700 -p .trustgraph-live/issue-actions-live
# Save composio-key here with owner-only permissions (chmod 600).
python3 scripts/check_composio_github.py \
  --repository your-account/demo-repository
cargo run -p trustgraph-composio-example -- --config trustgraph-actions-live.local.json --check
cargo run -p trustgraph-composio-example -- --config trustgraph-actions-live.local.json
```

The read-only preflight selects the correct credential scope:

- **Connect consumer key (`ck_`)**: uses `x-consumer-api-key` with
  [Connect MCP](https://docs.composio.dev/docs/composio-connect). It discovers a
  sole active GitHub account (or accepts `--account`), verifies repository access
  and issues enabled, and pins the issue input schema's SHA-256 in the local
  config. `--check` verifies that same account and schema through the Rust
  gateway without creating an issue. Connect exposes no dated tool version;
  this is a schema binding, not a provider implementation-version pin.
- **Project key (`ak_`)**: uses `x-api-key` with the project API, needs an explicit
  `--account`, and pins the returned dated tool version. Use the Python preflight
  for this route; `--check` is specific to Connect MCP. Consumer and project
  accounts have [separate scopes](https://docs.composio.dev/kb/guide/consumer-project-boundaries-and-auth-selection).

The preflight writes an owner-only local configuration and creates no issue. Stop
the preview server before starting live mode on the same ports. Live mode uses
a separate state directory and capabilities; preview approvals cannot carry over.

The write adapter uses only `GITHUB_CREATE_AN_ISSUE` with structured arguments.
Connect runs fixed discovery and schema checks immediately before passing the
approved arguments and pinned account to `COMPOSIO_MULTI_EXECUTE_TOOL`. A failed
preflight is confirmed as no write attempted. The project route uses Composio's
[execute API](https://docs.composio.dev/reference/api-reference/tools/postToolsExecuteByToolSlug).
There is no arbitrary tool, proxy, natural-language write execution, redirect or
automatic mutation retry. A successful live receipt requires the expected
GitHub issue URL in the response. Unclear responses remain `OUTCOME_UNKNOWN`;
inspect GitHub and provider logs before taking further action.

## Other examples and checks

[Linear issues](https://docs.composio.dev/toolkits/linear) and
[Notion pages](https://docs.composio.dev/toolkits/notion) are possible follow-on
adapters. This branch implements GitHub issue creation only. The original
`story`, `start` and patch-generation workflows remain available.

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
python3 -m unittest discover -s scripts -p 'test_*.py' -v
```

Mock-provider tests cover the exact approved payload, agent/owner separation,
revisions, stale/expired approvals, restart recovery, Connect JSON/SSE transport,
schema/account mismatch rejection and ambiguous outcomes.
A real Composio/GitHub connection still requires the read-only preflight and a
human-approved live issue creation.

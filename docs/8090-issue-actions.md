# 8090 → TrustGraph → GitHub issue

8090's native agent proposes a GitHub issue. TrustGraph requires human approval
of its exact title, body, repository, connected account and pinned tool version.
Composio creates the approved issue. This path starts no Codex process and
generates no patch.

8090 supplies the Work Order and project skill; its native workspace runs the
[restricted client](../scripts/trustgraph_actions.py). TrustGraph owns the review
UI, publishing credential and durable journal. This example approves a submitted
action; it does not capture the native agent's earlier context or infer source
permissions. Work-order identifiers are caller-supplied references, not verified
8090 assertions.

## Preview

```sh
cargo build --locked -p trustgraph-cli
cp trustgraph-actions.example.json trustgraph-actions.local.json
./target/debug/trustgraph actions
```

Set `github_repository` in the ignored local config to your demo repository.
Open the private `operator.html` file printed at startup to establish an HttpOnly
operator session at [127.0.0.1:8084](http://127.0.0.1:8084). The separate agent
API is on port 8085; it has no approval or execution routes.

Set the real project and Work Order IDs in the [example](../examples/8090/github-issue.json), then submit:

```sh
python3 scripts/trustgraph_actions.py \
  --token-file .trustgraph-live/issue-actions-preview/agent-token \
  propose --file examples/8090/github-issue.json
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

Add [TrustGraph issue approval](../examples/8090/trustgraph-issue-skill.md) as a
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
Keep the project API key in the gateway's `COMPOSIO_API_KEY` environment, or in
an owner-only `composio-key` file in the live state directory. Never put it in a
prompt, work order, skill or committed file.

```sh
mkdir -m 700 -p .trustgraph-live/issue-actions-live
# Save composio-key here with owner-only permissions (chmod 600).
python3 scripts/check_composio_github.py \
  --account YOUR_CONNECTED_ACCOUNT_ID --repository your-account/demo-repository
./target/debug/trustgraph actions --config trustgraph-actions-live.local.json
```

The read-only preflight checks the account and tool schema, then writes a local
configuration pinned to the returned dated version. It creates no issue. Stop
the preview server before starting live mode on the same ports. Live mode uses
a separate state directory and capabilities; preview approvals cannot carry over.

The adapter uses only `GITHUB_CREATE_AN_ISSUE` with structured arguments through
Composio's [execute API](https://docs.composio.dev/reference/api-reference/tools/postToolsExecuteByToolSlug).
There is no arbitrary tool, proxy, natural-language execution, redirect or
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
revisions, stale/expired approvals, restart recovery and ambiguous outcomes.
A real Composio/GitHub connection still requires the read-only preflight and a
human-approved live issue creation.

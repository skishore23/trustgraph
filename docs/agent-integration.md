# Integrate a coding agent

Connect the calling agent to a running TrustGraph workspace. The caller delegates
a protected task; TrustGraph owns the fresh generation and publication boundary.
The MCP server is a local stdio adapter to the same HTTP API used by the live UI.
It does not run a separate fixture workspace.

## 1. Start TrustGraph

Follow [live setup](development.md), then build and start:

```sh
cargo build --locked -p trustgraph-cli
./target/debug/trustgraph start
```

Startup creates separate owner and agent capabilities in the configured state
directory. The owner opens the private launch file for human review. Give the
MCP adapter only the **agent-token** file. It must be a private regular file
with owner-only permissions; do not paste its value into a prompt or commit it.

## 2. Register MCP

For Codex, use absolute paths so the connection does not depend on its working
directory:

```sh
codex mcp add trustgraph -- /absolute/TrustGraph/target/debug/trustgraph mcp \
  --url http://127.0.0.1:8080 \
  --token-file /absolute/state/agent-token
```

Equivalent Codex configuration:

```toml
[mcp_servers.trustgraph]
command = "/absolute/TrustGraph/target/debug/trustgraph"
args = ["mcp", "--url", "http://127.0.0.1:8080", "--token-file", "/absolute/state/agent-token"]
```

Codex shares MCP configuration across its CLI and editor integrations. See the
[official Codex MCP guide](https://developers.openai.com/codex/mcp).
Other MCP clients can launch the same command and arguments as a stdio server.
The adapter accepts only a loopback HTTP origin, disables proxies and redirects,
and does not retry HTTP requests automatically.

## 3. Give the agent a workflow

> Inspect TrustGraph’s workspace. Submit the admitted payment-reconciliation
> work order with Risk context. Report the work ID and inspect its status.
> If source access blocks it, request a fresh repair from shared sources and
> inspect that new work item. Show the decision and human review URL. Test a
> proposed policy if needed, then hand the exact result to the reviewer.

The predefined task is [work-order.txt](../examples/bankcase/work-order.txt).
In department mode, arbitrary new agent text is rejected unless an operator
has already admitted that exact work order. This prevents the caller from
silently declaring previously restricted context to be public task input.

| Tool | Arguments | What happens |
| --- | --- | --- |
| `trustgraph_workspace` | `{}` | Reads current sources, policy and work items |
| `trustgraph_submit_work` | `{ "goal": "<admitted task>", "include_risk": true }` | Queues a fresh controlled run; returns `id` and `review_url` |
| `trustgraph_status` | `{ "id": "job-…" }` | Returns that job’s status, events, output and decision when available |
| `trustgraph_repair` | `{ "id": "job-…" }` | Starts a new run for a Risk-context task, excluding the investigation and old patch |
| `trustgraph_simulate` | See case below | Compares proposed and current policy; records evidence without publication |

Status is asynchronous: inspect until the job leaves queued/running. A blocked
result is a real policy outcome, not permission to work around the gateway.
There are no MCP tools for approval, publication, policy activation or source
permission changes. Agents can propose policy changes in simulation; the operator
controls activation and exact-version review.

## HTTP and CLI clients

Any client on the same machine may use the HTTP API directly with an
`Authorization: Bearer <agent-token>` header:

| Request | Body / response |
| --- | --- |
| `GET /api/work` | Workspace, sources, current policy and jobs |
| `POST /api/work` | `{ "goal": "<admitted task>", "include_risk": true }` → `{ "id": "job-…" }` |
| `POST /api/work/{id}/repair` | `{}` → new job ID |
| `POST /api/simulate` | Supported scenario and optional proposed policy → comparison |

The CLI resolves the default token path from ignored `trustgraph.local.json`:

```sh
./target/debug/trustgraph run --include-risk \
  --goal "$(cat examples/bankcase/work-order.txt)"
./target/debug/trustgraph policy-simulate \
  --case examples/payment-change.json --policy examples/payment-policy.json --json
```

Example simulation tool arguments, with no proposed policy supplied:

```json
{
  "paths": ["src/lib.rs"],
  "tests_passed": true,
  "lineage_complete": true,
  "approval": "valid",
  "approval_age_minutes": 20,
  "include_risk": false,
  "audience": "engineering"
}
```

Add `policy` with the full [live policy](policy.md) object to test a proposed policy.
These are hypothetical assumptions; this call does not run tests or code.

## What the connection guarantees

The adapter returns actual live API results. The server captures a fresh Codex
run’s exact supplied context, records its output and checks source inheritance.
It does **not** accept arbitrary caller patches or a caller’s assertion that
lineage is complete. Previous messages in the outer coding agent are not captured
just because MCP is connected. To govern arbitrary existing agent runs, their
context and tool executions would need broker integration too.

Keep GitHub publication credentials in the gateway. A caller with independent
write credentials can bypass this local boundary. Remote agents need a reachable,
authenticated deployment or an on-host integration; this loopback adapter does
not create remote connectivity.

Submit work/repair once. If a transport reply is lost, inspect workspace/status
before resubmitting: task intake has no idempotency-key contract. The adapter
never silently retries a mutation. The runtime’s publication journal separately
handles stable operation keys and uncertain external outcomes.

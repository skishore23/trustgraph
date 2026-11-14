# Try TrustGraph through MCP

The running Rust app serves an authenticated Streamable HTTP MCP endpoint at `/mcp`. `trustgraph mcp` is a stdio bridge to that app, implemented with the official Rust SDK, pinned to rmcp 3.10.0. The UI and MCP share one coordinator, journal, compiler, prepared artifacts and delivery state. The bridge never opens the journal.

## Start the app

From the repository:

```sh
cargo build --locked -p trustgraph-cli
./target/debug/trustgraph serve --data .trustgraph-bank --port 8080
```

Keep it running. A private `mcp-token` is created in the data directory. This is a local capability for the synthetic **agent**, not a reviewer credential. Do not paste it into prompts. The bridge reads it from its explicit file path; it never prints it. Existing data directories and reviews are preserved.

## Add it to your client

Use [examples/mcp.local.json](../examples/mcp.local.json) on this machine, or use **Connect your agent → Download MCP configuration** in the web app. The download contains absolute executable/token-file paths and the current app port, but no secret. Merge the `trustgraph` entry into your client's existing configuration; do not overwrite other servers. Clients use different settings formats, so use the command/arguments in the format your client supports. Nothing in this implementation edits your client settings automatically.

The underlying stdio command is:

```sh
/Users/kishore/TrustGraph/target/debug/trustgraph mcp \
  --url http://127.0.0.1:8080/mcp \
  --token-file /Users/kishore/TrustGraph/.trustgraph-bank/mcp-token
```

Your client launches this process. Launching it in a terminal by itself waits for MCP input; it is not an interactive chat. Start `serve` first. When you use another port or directory, regenerate the configuration from that app. On another machine, change both absolute paths. This slice accepts only loopback HTTP `/mcp` upstream URLs and disables proxies, redirects and automatic request retries.

## Trial prompt

> Use TrustGraph to inspect the available sources and audiences. Simulate a memo using all three evidence documents for the Engineering repository. Explain the specific source/recipient conflict. Propose the returned typed plan, choose a fresh-context repair that keeps the Engineering audience, and prepare the repaired plan with the current plan/snapshot digests and a stable key. Return the human review link at http://127.0.0.1:8080 plus review_path. Stop and wait for my review; do not approve or deliver anything. Be explicit that the data and generation are synthetic.

Open the returned path in the app as Maya, review the actual memo, then **Approve & release** if appropriate. Ask the agent to check status afterward. MCP tool-call confirmation is not this content approval. Delivery remains simulated.

## Tool contract

| Tool | Input | Effect |
| --- | --- | --- |
| `trustgraph_workspace` | `{}` | Available source content, configured audience IDs and explicit synthetic agent scope |
| `trustgraph_simulate` | `sources`, `audience` | Stateless consulting preview; returns typed `plan`, matrix, conflict and `report` digests |
| `trustgraph_propose` | `plan` | Saves an immutable proposal; returns current analysis, repairs and runs |
| `trustgraph_analyze` | `id` | Current saved-plan analysis; no generation |
| `trustgraph_repair` | `id`, `kind` (`fresh_context` or `narrow_audience`) | Saves a recompiled alternative, never changes ACLs |
| `trustgraph_prepare` | `id`, `plan_digest`, `snapshot_digest`, `key` | Generates declared local-fixture inputs and creates a human review; returns `run` and `review_path` |
| `trustgraph_status` | `request_id` | The agent's actual review, content, feedback and observed outcome |

Always use the most recently returned plan ID and digests. Reuse the same key when retrying the same preparation. If a reply is lost, inspect analysis/runs or status before retrying; do not manufacture another key. Same-key retries reuse the same review. Stale digests and blocked plans fail before generation. Domain failures use caller-visible MCP error results; invalid arguments and unknown tools use protocol errors. Results include structured JSON and a text representation for compatible clients.

Read-only tools are annotated as such. Propose, repair and prepare are additive action tools, marked idempotent under their required identities/keys. No tool can choose another actor, approve, widen source rights, invoke arbitrary API routes, or publish. The existing reviewer app still uses fixed local demo identities; this is not production user authentication or isolation from a malicious local process.

## Limits and qualification

This is a working MCP interface to synthetic documents, fixture generation and local publishing. It does not import real documents or verify their ACLs; it does not capture another agent's conversations, tools or memory; it does not gate unrelated MCP calls or shell actions. A real deployment needs identity, authoritative source/use permissions, complete generation context and a controlled non-bypassable delivery adapter. See [8090 integration](8090-integration.md).

The implementation uses the [official Rust SDK](https://github.com/modelcontextprotocol/rust-sdk) for framing, initialization and transport. Tests exercise raw legacy-version HTTP MCP messages and an actual stdio child process using an SDK client, including a deliberately lost post-commit preparation reply. These tests establish the stated local contract, not an external client's or 8090 account's live qualification.

The bank policy extension adds `trustgraph_policies`, `trustgraph_policy_propose` and `trustgraph_policy_test`. See [policies](policies.md) and the current [bank trial prompt](bank-demo.md). The older consulting scenario remains available with `--scenario consulting`; default `serve` selects the bank.

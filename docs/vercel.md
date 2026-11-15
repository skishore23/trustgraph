# Public Vercel demo

[Open the demo](https://trustgraph-demo.vercel.app). No sign-in or API key is required.

The root page is a typed-dataflow lab backed by `trustgraph-framework` through
`POST /api/framework`. It reconstructs bounded fictional document/email and data
export pipelines, displays exact artifact and operation records, and checks type
compatibility, inherited access and approval freshness using Rust. Browser
controls are hypothetical inputs, not authority for a live effect. Each request
is independent; no session or persistent graph is needed.

The previous five-chapter Northstar story remains at `/story.html` (also
`/legacy.html`) with its existing `/api/demo` API. Both demonstrations are
fictional. Neither runs Codex, connects to HydraDB, sends mail, uploads exports,
or creates GitHub issues or pull requests.

## Deploy

From the repository root, with Rust 1.90+, Node.js, and an authenticated Vercel CLI:

```sh
cargo test --workspace --locked
node scripts/test_demo.cjs
node scripts/test_build_demo.cjs
node web/framework.test.cjs
vercel deploy --prod
```

`vercel.json` builds the static interface into `public/` with
`scripts/build_demo.cjs`, which empties `public/` first and fails if an expected
HTML anchor is missing. Vercel builds four Rust functions with its official Rust
runtime:

| Function | Route | Purpose |
|---|---|---|
| `api/framework.rs` | `POST /api/framework` | Typed-dataflow lab (root page) |
| `api/demo.rs` | `POST /api/demo` | Previous Northstar story (`/story.html`) |
| `api/simulation.rs` | `POST /api/simulation` | Seeded virtual-time simulator |
| `api/mcp.rs` | `/mcp`, `/api/mcp` | Read-only remote MCP server |

No external services are required. The only optional environment variable is
`TRUSTGRAPH_ALLOWED_HOSTS` (see [Remote MCP](#remote-mcp)).

Every endpoint answers errors with a JSON body, `{"error": "..."}`, including
malformed JSON (400), unknown fields (422), a missing JSON content type (415),
oversize bodies (413), wrong methods (405) and unknown paths (404). The
browser clients also tolerate non-JSON bodies from proxies and show a short
readable message instead of a parse error.

For local development with Vercel's routing and runtime bridge:

```sh
vercel dev
```

For the framework UI and Rust endpoints without Vercel credentials:

```sh
cargo run --locked --example framework_server
# http://127.0.0.1:9082
```

## Previous story: visitor progress

Each tab keeps its successful demo commands in `sessionStorage`. Requests to
`POST /api/demo` replay that history into a fresh in-memory `Story`, then execute
the next command or compare policies. Every step is revalidated by Rust. This
works across cold starts and concurrent instances without server-side sessions
or a shared database. Reloading preserves the tab's progress; **Restart the
story** clears it. Closing the tab normally clears its session storage. If
browser storage is unavailable, progress lasts until the page reloads. If the
server rejects a saved history (for example after a deployment changed the
story), the page clears it, restarts the story once and says so.

Requests are limited to 128 KiB and 64 commands. Restart the story to begin
another walkthrough. Failed commands are not added to browser history.

Client history is editable hypothetical demo input. These simulated approvals
never authorize a live publisher. The local `story`, live `start`, and [Composio issue example](../examples/integrations/composio-github-issues/README.md)
workflows retain their existing access controls and storage.

## Remote MCP

The public Streamable HTTP endpoint is:

```text
https://trustgraph-demo.vercel.app/mcp
```

It exposes two read-only tools:

- `trustgraph_demo_workspace`: inspect the fictional company and default policy.
- `trustgraph_demo_simulate`: compare policies against hypothetical source,
  destination, approval, and lineage assumptions.

No authentication or API key is needed. The endpoint is stateless, uses JSON
responses, accepts at most 32 KiB per request, and cannot access local runtime
files, credentials, HydraDB, Codex, Composio, or GitHub. It does not expose the
live gateway's task submission, review, policy activation, or publication tools.

To limit DNS rebinding and cross-site use, the endpoint accepts only requests
whose `Host` is `localhost`, `127.0.0.1`, `trustgraph-demo.vercel.app` or one of
Vercel's `VERCEL_URL`, `VERCEL_PROJECT_PRODUCTION_URL` and `VERCEL_BRANCH_URL`
values. A browser `Origin`, when present, must be `https://` one of those hosts.
To serve the endpoint from a custom domain, set a comma-separated list in the
Vercel project:

```sh
vercel env add TRUSTGRAPH_ALLOWED_HOSTS   # e.g. demo.example.com,trust.example.org
```

Other hosts and origins receive `403` with a JSON error.

Connect a Codex client without replacing its existing local TrustGraph server:

```sh
codex mcp add trustgraph-demo --url https://trustgraph-demo.vercel.app/mcp
```

Example request to `trustgraph_demo_simulate`:

```json
{
  "scenario": {
    "inputs": ["directory", "compensation"],
    "destination": "engineering",
    "approval": "valid",
    "complete_lineage": true
  }
}
```

The Rust verifier blocks this case because Engineering cannot access the
compensation source, even under an assumed valid approval. Use
`["directory", "approved-planning"]` to test the shared-input path.

## Seeded simulator

`POST /api/simulation` runs one bounded scenario from `trustgraph-sim` with a
16-bit seed. It returns ordered virtual-time events and local publication
receipts, never external effects. This exercises the existing coordinator, not
the generic SDK or arbitrary fault injection. The browser exposes all eight
predefined scenarios and shows the Rust-returned trace.

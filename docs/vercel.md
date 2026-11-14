# Public Vercel demo

[Open the demo](https://trustgraph-demo.vercel.app). No sign-in or API key is required.

The Vercel deployment presents the five-chapter Northstar company story. Its
Rust API uses the same policy compiler, lineage verifier, approvals, and simulated
publisher as the local guided demo. Data and generated artifacts are fictional;
releases are simulated. It does not run Codex, connect to HydraDB, or create
GitHub issues or pull requests.

## Deploy

From the repository root, with Rust 1.90+, Node.js, and an authenticated Vercel CLI:

```sh
cargo test --workspace --locked
node scripts/test_demo.cjs
vercel deploy --prod
```

`vercel.json` builds the static interface into `public/` with
`scripts/build_demo.cjs`. Vercel builds `api/demo.rs` using its official Rust
runtime. No environment variables or external services are required.

For local development with Vercel's routing and runtime bridge:

```sh
vercel dev
```

## Visitor progress

Each tab keeps its successful demo commands in `sessionStorage`. Requests to
`POST /api/demo` replay that history into a fresh in-memory `Story`, then execute
the next command or compare policies. Every step is revalidated by Rust. This
works across cold starts and concurrent instances without server-side sessions
or a shared database. Reloading preserves the tab's progress; **Restart the
story** clears it. Closing the tab normally clears its session storage. If
browser storage is unavailable, progress lasts until the page reloads.

Requests are limited to 128 KiB and 64 commands. Restart the story to begin
another walkthrough. Failed commands are not added to browser history.

Client history is editable hypothetical demo input. These simulated approvals
never authorize a live publisher. The local `story`, live `start`, and issue
`actions` workflows retain their existing access controls and storage.

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

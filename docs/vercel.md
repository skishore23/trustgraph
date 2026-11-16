# Public Vercel demo

[Open the demo](https://trustgraph-demo.vercel.app). No sign-in or API key is required.

The root page is the **policy playground**: an editable `trustgraph.policy.yaml`
(starting from the six bundled examples), an agent run to decide, and the
verdict with its reason, the blocking sources and who may read the output. It also shows the
policy's inline test results, the plain-English explanation, the diff against
the example as loaded (with every widening flagged) and a seeded simulator run.
Everything is computed by `trustgraph-policy` and the policy runtime through
`POST /api/policy`. Each request is independent; no session, storage or
external service is involved, and the policy text is sent nowhere else.

The earlier pages stay reachable by link:

- `/lab.html`: the typed-dataflow lab backed by `trustgraph-framework` through
  `POST /api/framework` (fictional document/email and data-export pipelines).
- `/story.html` (also `/legacy.html`): the five-chapter Northstar story with its
  `/api/demo` API.

All demonstrations use fictional data. None runs Codex, connects to HydraDB,
sends mail, uploads exports, or creates GitHub issues or pull requests.

## Deploy

From the repository root, with Rust 1.90+, Node.js, and an authenticated Vercel CLI:

```sh
cargo test --workspace --locked
node scripts/test_demo.cjs
node scripts/test_build_demo.cjs
node web/framework.test.cjs
node web/site.test.cjs
vercel deploy --prod
```

`vercel.json` builds the static interface into `public/` with
`scripts/build_demo.cjs`, which empties `public/` first and fails if an expected
HTML anchor is missing. It publishes `web/index.html` as `/index.html` and
`web/framework.html` as `/lab.html`. Vercel builds five Rust functions with its
official Rust runtime:

| Function | Route | Purpose |
|---|---|---|
| `api/policy.rs` | `POST /api/policy` | Policy playground (root page) |
| `api/framework.rs` | `POST /api/framework` | Typed-dataflow lab (`/lab.html`) |
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

For the playground, the story and every Rust endpoint without Vercel
credentials (pages are read from `web/` on each request, so edits show on
refresh; `/lab.html` is created by the build and is not served here, so use
`vercel dev` to see the lab):

```sh
cargo run --locked --example framework_server
# http://127.0.0.1:9082
```

## Policy playground: `/api/policy`

`POST /api/policy` takes one JSON operation per request, chosen by `op`:
`examples`, `check` (diagnostics, inline test results, explanation and a
summary of names), `decide` (one query: sources read, agent, action and
optional explicit recipients), `diff` (old and new policy text) and `simulate`
(seeded runs of the policy runtime with invariant checks and path coverage).
A policy with problems is not an HTTP error: the response is 200 with
`ok: false` and its diagnostics. Malformed requests get 400 with
`{"error", "code"}`.

Limits, which are rejected rather than clamped: a 128 KiB body, 64 KiB of
policy text, 64 sources and 256 recipients per query, names up to 256 bytes,
and for `simulate` a seed from 0 to 65535, `count` 1 to 8 (default 4) and
`steps` 1 to 120 (default 80). The simulator only runs a policy that is valid
and passes its own inline tests, as activation would require. Each operation
runs off the async executor, and a panic becomes a 503 JSON error.

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

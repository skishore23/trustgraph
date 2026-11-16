# Public Vercel site

[Open the site](https://trustgraph-demo.vercel.app). No sign-in or API key is required.

The site is one page, the **policy playground**: an editable `trustgraph.policy.yaml`
(starting from the bundled examples), one-click experiments, an agent run to
decide, and the verdict with its reason, the blocking sources and who may read
the output. It also shows the policy's inline test results, the plain-English
explanation, the diff against the example as loaded (with every widening
flagged) and a seeded simulator run. Everything is computed by
`trustgraph-policy` and the policy runtime through `POST /api/policy`. Each
request is independent; no session, storage or external service is involved,
and the policy text is sent nowhere else. The examples use fictional people.

## Deploy

From the repository root, with Rust 1.90+, Node.js, and an authenticated Vercel CLI:

```sh
cargo test --workspace --locked
node scripts/test_build_demo.cjs
node web/site.test.cjs
vercel deploy --prod
```

`vercel.json` builds the static site into `public/` with
`scripts/build_demo.cjs`, which empties `public/` first and publishes
`web/index.html`, `web/site.css` and `web/site.js`. Vercel builds two Rust
functions with its official Rust runtime:

| Function | Route | Purpose |
|---|---|---|
| `api/policy.rs` | `POST /api/policy` | Policy playground |
| `api/mcp.rs` | `/mcp`, `/api/mcp` | Read-only remote MCP server |

No external services are required. The only optional environment variable is
`TRUSTGRAPH_ALLOWED_HOSTS` (see [Remote MCP](#remote-mcp)).

Every endpoint answers errors with a JSON body, `{"error": "..."}`, including
malformed JSON (400), a missing JSON content type (415),
oversize bodies (413), wrong methods (405) and unknown paths (404). The
page also tolerates non-JSON bodies from proxies and show a short
readable message instead of a parse error.

For local development with Vercel's routing and runtime bridge:

```sh
vercel dev
```

For the page and `/api/policy` without Vercel credentials (files are read
from `web/` on each request, so edits show on refresh):

```sh
cargo run --release --locked --example site_server
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
`steps` 1 to 120 (default 80); the page asks for 8 runs of 120 steps. The simulator only runs a policy that is valid
and passes its own inline tests, as activation would require. Each operation
runs off the async executor, and a panic becomes a 503 JSON error.

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

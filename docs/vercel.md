# The public site

[Open the site](https://trustgraph-demo.vercel.app). No sign-in or API key is
required.

## What is deployed

One page, the policy playground (`web/index.html`, `web/site.css`,
`web/site.js`), and one Rust function, `api/policy.rs`, which serves
`POST /api/policy`. Every result on the page comes from `trustgraph-policy`
and the policy runtime through that endpoint. Each request is independent: no
session, storage or external service is involved, and the policy text is sent
nowhere else. The examples use fictional people.

## The endpoint

`POST /api/policy` takes one JSON operation per request, chosen by `op`:
`examples`, `check` (diagnostics, inline test results, explanation and a
summary of names), `decide` (one query: sources read, agent, action and
optional recipients), `diff` (old and new policy text), `simulate` (seeded
simulator runs) and `run` (up to 6 agent steps, each reading sources or earlier
steps' outputs, driven through a fresh in-memory `trustgraph-engine`; returns
what each step recorded or the engine's refusal, then the decision for one
action). `run` activates the draft without its inline tests, so a draft whose
tests fail can still be tried; `trustgraph serve` refuses to activate one. A policy with problems is not an HTTP error: the answer is
200 with `ok: false` and its diagnostics.

Limits, rejected rather than clamped: a 128 KiB body, 64 KiB of policy text,
64 sources and 256 recipients per query, names up to 256 bytes (64 for `run`,
with at most 8 reads per step), and for
`simulate` a seed from 0 to 65535, `count` 1 to 8 (default 4) and `steps` 1 to
120 (default 80). The simulator runs only a policy that is valid and passes its
own tests. Every error is a JSON body, `{"error": "..."}`: malformed JSON
(400), a missing JSON content type (415), an oversize body (413), a wrong
method (405) and an unknown path (404). A panic becomes a 503 JSON error.

## Build and deploy

`vercel.json` runs `node scripts/build_demo.cjs`, which empties `public/` and
copies the page, its two assets and the self-hosted IBM Plex fonts
(`web/fonts/`, SIL OFL 1.1) into it. Vercel builds `api/policy.rs` with
its Rust runtime. No environment variables are needed.

```sh
cargo test --workspace --locked
node scripts/test_build_demo.cjs
node web/site.test.cjs
vercel deploy --prod
```

## Run it locally

Without Vercel, see [development](development.md#run-the-site-locally). With
Vercel's routing and runtime bridge:

```sh
vercel dev
```

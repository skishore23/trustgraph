# TrustGraph staged implementation plan

> Execute with the executing-plans skill in this session. Track completed stages
> here and retain meaningful verification output under evidence/.

**Goal:** Deliver the Northstar governance loop in Rust with deterministic failure testing.

**Architecture:** Pure verification over materialized evidence; a single-owner
runtime controls mutations and external effects. Simulation changes environment
ports rather than implementing another policy engine.

**Tech stack:** Rust 2024, Serde, SHA-256, Axum/Tokio, Reqwest, proptest; resolved
dependencies pinned in Cargo.lock after the build smoke test.

**Spec:** [Design](../specs/2026-11-13-trustgraph-design.md).

## Global constraints

- Synthetic one-workspace data; company readership does not include the Internet.
- Do not copy source or wording from the inspiration repository.
- Missing, ambiguous or inconsistent evidence cannot authorize a post.
- Do not claim a real Hydra/model/publisher integration from fixture tests.

## Review focus

Changed bytes; private prior drafts hidden in fresh-looking context; null and
truncated graph results; restart between dispatch and acknowledgment; replayed
operation keys carrying a changed destination. Each belongs to the tests below.

## Stage 1 — Decision library

Files: core/src/{domain,digest,lineage,verifier,bundle}.rs; core/tests/governance.rs.
Interface: `verify(&Evidence, &Intent, now: u64) -> Decision`.

- [x] Write executable negative and valid-path fixtures; observe behavioral red.
- [x] Implement typed IDs, immutable records, scope/reference equations, bounded
  closure, source intersections, canonical contexts and approval validation.
- [x] Test content/destination/audience/policy/ACL changes, revoked/expired
  authority, cycles, private previous drafts, incomplete manifests and tampering.
- [x] Add finite reader-set algebra laws with independent expected results.

## Stage 2 — Controlled runtime

Files: runtime/src/{engine,journal,publisher,fixtures}.rs and tests. Generation is
implemented inside the coordinator rather than a separate adapter module.
Interface: typed coordinator methods `generate`, `edit`, `approve`, `publish` and
`reconcile`. Publication operates by immutable operation key and expected context digest.

- [x] Fail tests for duplicate dispatch, lost acknowledgment, recovery and stale
  verification; implement durable write-ahead state and controlled publisher.
- [x] Implement broker-created manifests and clean generation without old context.
- [x] Enforce authenticated role boundaries on approvals, source/policy changes
  and evidence reads; filter private metadata from responses.
- [x] Verify disk reopen, torn records, journal integrity and exclusive ownership.

## Stage 3 — Small simulator

Files: sim/src/{lib,trace}.rs; sim/tests/scenarios.rs.
Interface: `run(seed, scenario) -> Trace`; versioned trace stores virtual steps.

- [x] Fail scenarios for the two demo blocks and ambiguous publisher outcomes.
- [x] Run production coordinator under virtual time with scheduled ACL changes,
  approval revocation, publication, provider faults and process reopen models.
- [x] Campaign over bounded seeds; require zero forbidden publications and
  reproducible trace equality. Add deliberate policy mutant sensitivity check.

## Stage 4 — CLI and local review flow

Files: cli/src/{main,api}.rs; web/index.html; README.md.

- [x] Implement demo, simulate, replay, serve and receipt export commands.
- [x] Exercise the review flow through actual HTTP requests, including unauthorized
  approval and evidence-read requests. Verify published bytes equal approved bytes.
- [x] Provide source/draft/review/receipt screens, clear synthetic labels and a
  reset command that uses a fresh data directory rather than deleting old evidence.

## Stage 5 — Hydra evidence projection

Files: runtime/src/hydra.rs; runtime/tests/hydra_contract.rs; docs/hydra.md.

- [x] Check current server documentation for typed cells, pagination, bookmarks,
  node upsert and mutation options before implementing queries.
- [x] Parameterize all values, register numeric node identities explicitly,
  preserve incidence nodes, and reject incomplete reads or bundles.
- [x] Test protocol parsing against HTTP fixtures. Supply opt-in real-server
  roundtrip tests.
- [ ] Qualify live Hydra behavior against an actual server; the local connection
  attempt failed and the real contract test remains explicitly ignored.

## Completion checks

- [x] `cargo fmt --all --check`
- [x] `cargo clippy --workspace --all-targets --locked -- -D warnings`
- [x] `cargo test --workspace --locked`
- [x] CLI demo, simulator campaign, exported replay and HTTP acceptance path.
- [x] Record implemented vs unqualified stages, run instructions and limits.

## Implementation rulings

Generation stays in the small coordinator for this fixture adapter; there is no
unused provider abstraction. The verifier uses focused lineage and bundle modules
with policy/approval predicates together in verifier.rs. The API surface uses
inline candidate requests rather than persisted intent CRUD, and initializes only
the fixed synthetic sources. Source import, a real model adapter and graph-backed
evidence reads remain in the next-stage plan.

The independent final review found a memoized shared-subtree depth-limit bug. A
behavioral red regression reproduced it; cached subtree height now enforces the
same bound for every path regardless of input order. All completion checks below
refer to the final implementation rather than the earlier milestone test counts.

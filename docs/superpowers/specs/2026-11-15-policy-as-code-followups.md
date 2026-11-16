# Policy-as-code: follow-ups after part 1

Recorded 2026-11-15 from the task reviews and the whole-branch review of part 1
(`docs/superpowers/plans/2026-11-15-policy-as-code-part1.md`). Nothing here
blocks part 1. Items are grouped by where they belong.

## Must be settled in part 2 (`trustgraph serve`)

These become real the moment agents can reach the runtime over HTTP or MCP.

- **Per-agent quotas.** The journal, artifacts, Graph operations (one per
  distinct step name) and lineage all grow with agent calls and nothing caps
  them. Add quotas before exposing `serve`.
- **Operator identity.** `ActivatePolicy.operator` and `ProposePolicy` with no
  agent are unauthenticated strings inside the engine. The design says the
  operator is one named person who must be a listed reviewer. Enforce it,
  preferably inside the engine so replay checks it too.
- **Which policy is in force.** After an in-service activation, a restart with
  the original file keeps the activated policy. Add a `Runtime` accessor that
  says whether the file or an in-service activation is in force, and report it
  at startup and on the operator page.
- **Journal durability and size.**
  - `load` reads the whole journal into memory; replay time grows with history.
    Add streaming replay, then snapshots or compaction.
  - Add a journal format version; today a journal is only guaranteed to replay
    on the engine version that wrote it.
  - macOS needs `F_FULLFSYNC` for the durability claim to hold.
  - Tighten an existing state directory to 0700; add `deny_unknown_fields` to
    `Record`.
  - Pre-check a changed policy file on a throwaway engine in the non-empty
    journal case too, so a crash-looping service cannot append one rejected
    Genesis per restart.
  - Add a fault hook so the poisoned-journal wiring can be tested with a real
    injected `sync` / `set_len` failure.
- **Suggestion cost.** A policy proposal with ~100 unknown names against
  thousands of same-length names costs about a second of CPU. Fine for a CLI,
  not for an agent-callable endpoint: bound it or rate-limit proposals.
- **API shape.** Decide for `serve`: `Check` on an unknown artifact is a
  rejection, not a decision; a rejected `RecordOutput` can leave a registered
  Graph operation; a Genesis written by a restart records no operator.
- **JSON everywhere.** Argument-parser errors are plain text even under
  `--json`; MCP bypasses the parser, but the CLI contract should be uniform.
- **State directory docs** for the Composio example once `serve --state`
  exists; rewire that example to call `serve` for its decisions.

## Should be done in part 2 or 3 (quality)

- Diagnostic paths are ambiguous when a name contains a dot
  (`groups.a.b`); matters for machine consumers. Consider bracket quoting.
- Duplicate-key detection matches serde_yaml's English message; pin it with a
  test. serde_yaml 0.9 is unmaintained: plan a replacement.
- Round-trip (`to_yaml` → `parse`) is tested on one policy; add the five
  examples and a policy with explicit recipients and quoting-sensitive names.
- Validate the starter and examples against `policy schema` (needs a JSON
  Schema validator as a dev-dependency); the schema test compares key names
  only.
- `diff` reports a removed inline test only as "tests changed"; name the
  removed tests, since removing a deny test weakens the safety net.
- A did-you-mean for an invalid `expect` value.
- Lint tests assert a code is present, not that it is the only one.
- `test` and `explain` hide warnings in human mode; `check` shows them.
- Simulator: assert coverage per example, not only combined; a test that fires
  the "decide disagrees with policy text" cross-check; a test that evicts a
  remembered decision holding a live approval; a long run that reaches the
  1,024-decision cap.
- Public API: `decide`, `diff`, `lint`, `explain` are reachable both as modules
  and as re-exports; pick one path before external embedders arrive.

## Known and accepted

- Removing trailing journal records, or deleting the journal, is not
  detectable: there is no external head anchor. The file must be protected by
  filesystem permissions and kept outside agent workspaces. (An anchor could be
  added later.)
- Every policy activation voids all approvals and pending decisions, even for
  an unrelated edit. Stricter than the design requires; fails closed.
- Plain `policy init` needs hard-link support and fails with an error on
  filesystems without it; `init --force` replaces the file, so an existing
  file's mode is not preserved.
- The strict intersection rule means one sensitive input restricts the whole
  output. A human-approved, audited "release" exception is the likely first
  feature request after part 3.

## Not yet verified

- The four Kani harnesses in `crates/trustgraph-core/src/decision.rs` have not
  run: Kani is not installed locally. The `Bounded permission proofs` workflow
  runs them on a pull request or a push to `main` that touches
  `crates/trustgraph-core`. Record the result in `docs/verification.md` only
  after that run.

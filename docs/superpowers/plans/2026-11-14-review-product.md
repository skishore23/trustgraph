# Agent review inbox implementation plan

Spec: [Product design](../specs/2026-11-14-review-product.md).
Execute natively using the existing Rust verifier and coordinator.

- [x] Add journaled review requests, feedback and immutable revision history in
  runtime/model.rs and runtime/submissions.rs. Write failing behavior tests for
  idempotency, actor roles, feedback/approval revocation, revision fencing, reopen
  and one-operation release before implementing.
- [x] Expose permission-filtered request list/detail and submit/revise/return/release
  routes in cli/inbox.rs. Seed clearly labeled sample requests through production
  methods. Verify actual HTTP intake and review actions.
- [x] Replace the demonstration page with a request-centered application shell:
  searchable inbox, request review page, releases, sources and configured policy.
  Use functional navigation, accessible forms/dialogs, current check results and
  safe text rendering. Keep local-preview controls separate from normal work.
- [x] Exercise the app in the browser, including blocked requests, feedback and
  exact-context release. Inspect layout and capture a review/inbox screenshot.
- [x] Run formatting, Clippy and workspace tests. Record product choice, executed
  checks, remaining real-agent integration boundary and screenshot artifact.

Implemented: the agent review inbox is the normal product surface; the earlier
drafting demo remains available as a CLI scenario. Request metadata, feedback and
revision histories live in the authoritative journal. The final review identified
and fixed retry authorization and pre-dispatch request linkage gaps, each with an
observed red regression followed by green tests.

Verification: 44 tests passed, one live Hydra test explicitly ignored, Clippy and
formatting clean. Browser feedback/release/navigation/search flows passed. The
final screenshot is `evidence/product-inbox.jpg`; API usage is documented in
`docs/agent-intake.md`.

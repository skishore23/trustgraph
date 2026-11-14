# Northstar Bank: one agent, two departments

The guided demo runs a separate, in-memory sandbox alongside the persistent
workspace. It uses the same Rust lineage verifier, immutable artifact generation,
editing and human-approval commands. Reset discards only sandbox history.
Documents, patches and reports are fictional fixtures. No model, test runner,
branch push or GitHub PR is executed by the walkthrough.

## Implemented stages

1. Record the three chapters as real source/manifest/artifact relationships and
   test the information boundary and exact approval binding.
2. Traverse actual input edges forward for hypothetical access changes. Mark
   affected work before reevaluating it. Keep the active workspace untouched.
3. Add a guided app view: short business context, clear actions, dependency map,
   independent permission/approval results and inspectable evidence.
4. Exercise the API and browser, run workspace checks and check in the demo.

## Run of show

Run `cargo run --locked -p trustgraph-cli -- serve --scenario bank --port 8080`
and open `http://127.0.0.1:8080/#/demo`. Click **Reset demo** before presenting.
The walkthrough uses its own fixed demo policy; Policy studio manages the
persistent workspace, not this sandbox. Restarting the server also resets the
sandbox. Baseline approvals last one hour; refresh them or reset for a later demo.

1. **The boundary (30 seconds).** Preview as Ravi (Risk owner; internal demo ID
   `arun`). Atlas has read the Engineering requirements and Risk investigation.
   Simulate publishing. Follow the blocked path into Engineering. Create a fresh
   Engineering-only generation. Its source permissions pass; approval is still
   required. No equivalence between the two outputs is asserted.
2. **Approval has a scope (40 seconds).** Preview as Maya. Inspect and approve
   patch v1. Change the patch, then simulate publishing: information is permitted,
   but approval no longer matches. Approve the new exact patch, simulate again.
   Changing the destination also requires a new approval. Original approvals stay
   in the sandbox's history; Maya's authority has not been revoked.
3. **What changed? (60–90 seconds).** Review the release map and approve the four
   baseline publication candidates. Restrict Case analysis to Risk in the
   hypothetical overlay. “See impact” marks descendants “Needs recheck”; then
   reevaluate. Regression work and the combined Engineering release are blocked.
   The Risk note still has a permitted audience but needs renewed approval.
   Standalone compatibility work and the independent documentation fix remain
   ready. Changing API specification instead demonstrates a different affected
   set. Shared outputs do not make their unrelated inputs descendants.

The last chapter models reports as generated input artifacts, not attestations
that tests executed or passed. Edges identify input use, exact-context approval
and intended destination. Permissions and approval validity are separate checks.
Only authorized preview identities can inspect restricted chapter-one details;
agents cannot approve work. Preview identities are local demo authentication.

## Live integration boundary

The persistent workspace already exposes HTTP, CLI and MCP agent submission,
review and policy workflows. Live HydraDB qualification, captured real model
inputs, Composio branch publication and draft-PR creation still need the service
details and end-to-end tests. The guided demo performs zero publisher calls and
must not be described as a live GitHub or software-factory run.

## Verification

`scripts/check.sh` passes: formatting, Clippy with warnings denied, and 76 tests
with zero failures. The single ignored test requires a live HydraDB endpoint.
The new runtime tests cover the fresh manifest, stale patch and destination,
forward-only impact on either source, independently preserved approval digests,
Risk-audience renewal and real approval expiration. The HTTP test verifies actor
authorization, exact review digests, reset and unchanged persistent journal bytes.

All three chapters were exercised in the browser, including the alternate API
specification change. The UI supports keyboard-accessible controls, explicit
“Needs recheck” and reevaluation stages, inspectable typed edges and reset.
Decision evidence is under `evidence/story-*`; no live publisher calls occurred.

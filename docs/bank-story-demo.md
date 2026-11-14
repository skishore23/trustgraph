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
and open `http://127.0.0.1:8080/#/demo`. Click **Reset** before presenting.
The walkthrough uses its own fixed demo policy; Policy studio manages the
persistent workspace, not this sandbox. Restarting the server also resets the
sandbox. Baseline approvals last one hour; refresh them or reset for a later demo.

1. **The boundary (30 seconds).** Preview as Ravi (Risk owner; internal ID
   `arun`). Atlas has read the Engineering requirements and Risk investigation.
   **Check permissions** shows the blocked path into Engineering. **Generate with
   safe inputs** starts an Engineering-only generation. Its source permissions
   pass; approval is still required. The two outputs are not claimed equivalent.
2. **Approval has a scope (40 seconds).** Preview as Maya. Inspect and **Approve
   version 1**. **Change patch**, then **Check approval**: information is permitted,
   but approval no longer matches. Approve the new exact patch. Original approvals
   stay in the sandbox history; Maya's authority has not been revoked. Exact
   destination binding remains covered by API tests.
3. **What changed? (60–90 seconds).** Review the four work items and **Approve
   baseline**. **Restrict case analysis to Risk** creates a hypothetical access
   overlay. Descendants show “Needs recheck”; **Recheck affected work** evaluates
   them. Regression work and the combined Engineering release are blocked. The
   Risk note is still permitted but needs renewed approval. Compatibility work and
   the independent documentation fix remain ready. Runtime tests cover a separate
   API-specification change and verify that impact never travels backward into
   unrelated inputs.

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

All three chapters were exercised in the browser. The UI supports keyboard
controls, explicit “Needs recheck” and reevaluation stages, inspectable typed edges and reset.
Decision evidence is under `evidence/story-*`; no live publisher calls occurred.

## Interface

The bank app has three main views: **Demo**, **Reviews** and **Policy**. The demo
uses **Information**, **Approval** and **Impact** chapter tabs, with one primary
action at each step. Destination management, source browsing, delivery history,
plan editing and bounded verification are absent from the bank navigation.
The underlying API and CLI remain available for integration work.

The Engineering audience is fixed in the walkthrough. Evidence and approval
history open on demand. The impact scene contains one compact dependency map and
four short outcome rows, with item details available by selecting a node.

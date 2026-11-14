# Build evidence

This directory records bounded checks for this implementation. Simulator results
apply to the shared Rust decision/coordinator code with declared environment
models. They do not qualify real Hydra deployments, live publishers, or LLMs.

## Selected plan compiler direction — November 14, 2026

The user supplied and selected the compiler-for-safe-agent-plans brief. Plans now
lead the application; reviews are a runtime obligation after actual generation.
The first local slice has Read/Generate/Approve/Publish contracts, current source
reader analysis, an exact recipient/source/path witness, two recompiled repairs,
durable preparation and dispatch-time plan/output/destination checks.

Verification: **58 tests passed, zero failed, one live Hydra contract ignored**.
`plan-final-checks.log` records formatting, Clippy with warnings denied and the full
workspace suite. The compiler/runtime/model initial red logs and green logs are
retained separately. JavaScript module syntax was checked with Node.

The independent review and targeted fault tests found and fixed: syntax errors
masking unreadable Read references, long Unicode goals exceeding review-title
bounds, and partially generated outputs escaping plan checks before preparation
completed. Every preparation commit boundary is tested with before/after-commit
faults. Generated artifact identities are reserved before generation, and source
read authorization is independent of the compiler's first diagnostic.

`bounded-correct.json` records the depth-6 finite model result: 103 nonterminal
states, 624 transitions, no counterexample under the production gate. The fixed
world and exclusions are explicit. `bounded-broken.json` records a shortest
four-action trace for the model-only approval-reuse fault. The CLI deliberately
returns status 1 on this counterexample; no production policy is changed.

The actual HTTP test and browser walkthrough completed unsafe proposal → concrete
counterexample → fresh-context alternative → tracked generation → human review of
actual bytes/destination → simulated publication. Browser checks also exercised
both normal and broken-rule verification views. The tab remains on the original
unsafe proposal so the two alternatives can be inspected. Screenshots:
[workspace](plan-compiler.jpg), [full flow and repairs](plan-compiler-full.jpg).

MCP transport, live Hydra plan persistence/validated reads, Composio dispatch and
optional Kani remain separate qualification stages. The legacy low-level review
API still exists; only plan-associated artifacts have plan-specific contracts.
Every publication continues to use the existing source/approval verifier. No
whole-system security or novelty claim is made.

## Review inbox product — November 14, 2026

The user selected existing agents submitting work for review as the primary
product flow. The UI now centers a persistent request queue and dedicated review
page, with release history and source/destination/policy views. No tenant or
billing surfaces were introduced.

Final product checks: **44 passing tests, zero failures and one ignored live
Hydra contract test** in `product-tests.log`; clean workspace Clippy in
`product-clippy.log`, formatting in `product-fmt.log`, and JavaScript syntax
checked with `node --check web/app.js`. The original eight-scenario, 64-seed
campaign and replay were repeated in `product-campaign.log` and
`product-campaign-replay.log`.

New runtime tests cover agent intake, idempotency, read scope, durable feedback,
approval revocation on return, agent resubmission, expected-revision fencing and
one-operation release across reviewers. The HTTP test exercises actual intake,
return, revision and release routes. `product-requests-red.log` preserves initial
behavioral failures and `product-requests-green.log` the first green stage.

The independent product review found two additional bugs. An original-submission
retry could return a later unreadable revision; request-to-release linkage could
be lost between separate journal commits. Both were reproduced in
`product-review-red.log` and corrected in `product-review-green.log`. Retry reads
now apply historical request authorization, and linkage is part of the durable
operation reservation before sending. Earlier journals replay consistently.

Browser verification completed source-access blocking, written feedback, persisted
changes-requested status, exact-content approval and simulated release, release
history, source/configuration navigation, queue search and attention filtering.
The native narrow view keeps decision actions in a fixed action bar.
[Inbox screenshot](product-inbox.jpg) records the final queue. The preview remains
open on the Inbox. Agent intake is restricted to broker-tracked artifact IDs;
external agent transport/authentication and real publishing remain unqualified.

The project brief supplied by the user is the requirements source. No text or
code from the inspiration repository is included in the application.

## Final verification — November 13, 2026

| Check | Observed result | Record |
| --- | --- | --- |
| Rust format | Exit 0 | fmt.log |
| Clippy, workspace/all targets, warnings denied | Exit 0 | clippy.log |
| Workspace tests, locked dependencies | 38 passed, 0 failed, 1 explicitly ignored | workspace-tests.log |
| Deterministic campaign | 512 runs: 8 scenarios × seeds 0–63 | campaign.log |
| Campaign replay | 512 traces matched regenerated executions | campaign-replay.log |
| Complete CLI demo | Permission block, stale approval, reapproval and one exact publication | demo.log |
| Exported decision replay | Historical authorization validated | bundle-replay.log |
| Deliberate permission-gate bypass in isolated copy | Behavioral test failed as intended; original checkout untouched | policy-mutation.log |
| Browser walkthrough | Restricted block, fresh generation, exact review, edit/stale block, reapproval, published simulator receipt | Manual via loopback review desk |
| Real Hydra server | Not available at 127.0.0.1:8443 | Real contract test remains ignored |

The ignored test is
`real_hydra_projection_roundtrip_is_idempotent_and_complete`; it requires an actual
server, token and allocated numeric ID range. Protocol fixture tests are included
in the 38 passing tests and do not replace it.

`toolchain.log` records Rust/Cargo versions. `Cargo.lock` records resolved
dependencies. Reproduce the gates with `sh scripts/check.sh`, the campaign with
the README commands, and mutation sensitivity with
`python3 scripts/check-policy-mutation.py`.

## Red-first milestones and review fixes

Initial stage logs preserve observed assertion failures followed by green runs.
Stage 1 began with a fail-closed placeholder and 11 expected behavior failures;
stage 2 with 8 failures at unimplemented coordinator methods; stage 3 with 4
scenario failures; stage 4 failed at the expected HTTP authorization status; stage
5 failed at missing transport behavior. Empty unit-test targets are not counted
as implemented behavior. Final workspace results supersede initial counts.

An older-version edit regression failed with a conflict before version allocation
was corrected (`edit-history-red.log`). Blob storage was observed absent before
implementation (`content-store-red.log`). The independent review found one
shared-DAG depth-cache bug, reproduced by `shared-depth-red.log` and fixed by
retaining subtree height. Both traversal-limit tests pass in the final suite.

The simulator covers bounded command schedules and explicit faults. Disk tests
cover real local reopen and integrity checks, not arbitrary process-kill or
hardware durability qualification. No real model or external publisher was used.


## Consulting decision workflow

- `consulting-checks.log`: formatting, warnings-as-errors Clippy, and the complete workspace test run: 60 passed, 0 failed, 1 ignored live Hydra contract.
- `consulting-simulator.jpg`: native 688px viewport, default blocked Atlas acquisition case.
- `consulting-simulator-full.jpg`: full simulator with the source/recipient table and both plan alternatives.
- `consulting-delivery.jpg`: approved local delivery retained across server restart.

Browser verification exercised the actual source checkboxes and destination choices, both repairs, preparation into human review, approval and simulated delivery, scoped Sam identity, and returned-plan guidance. The final screen is restored to Maya and the original blocked selection. No external publication occurred.


## MCP integration

- `mcp-workspace-tests.log`: full workspace suite, 65 passed, zero failed, one explicitly ignored live Hydra test.
- `mcp-token-test.log`: additional token persistence and owner-only permissions test, passed (66 distinct tests total).
- `mcp-clippy.log`: warnings-as-errors Clippy result.
- `mcp-local-smoke.json`: actual stdio trial against the running app, including blocked source/recipient, repaired plan and pending human-review path. No publication or 8090 account connection performed.
- `mcp-connection.jpg`: setup and configuration-download UI.
- `mcp-created-review.jpg`: actual memo prepared through MCP, waiting for a human.

The MCP tests also run an actual SDK child process against a faulting HTTP response after prepare commits. It creates exactly one review and never resends the ambiguous mutation. The local configuration download contains paths and a URL, never the capability secret.


## Small policy layer and bank scenario

- `bank-final-checks.log`: full workspace verification, 71 passed, zero failed, one intentionally ignored real-Hydra contract.
- `bank-baseline-report.json`, `bank-short-review-report.json`: both example policies' five-case reports, including actual compiler/verifier details.
- `bank-mcp-trial.json`: live local stdio proposal/test/repair/preparation against synthetic bank sources; no external publication or live-service qualification.
- `bank-policy-studio.jpg`, `bank-policy-studio-full.jpg`: actual policy studio with five passing cases; a second policy was saved and activated as revision 2.
- `bank-patch-review.jpg`: the repaired tracked Rust patch visible to Engineering, without the Risk investigation among its inputs.

The final browser is left in Policy studio as Arun. Source content is fictional. Real coding-agent model-context capture and Composio branch/PR publication remain next stages. The GitHub project check-in is separate from that publisher boundary.

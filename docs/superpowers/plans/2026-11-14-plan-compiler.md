# Safe plan compiler implementation plan

Spec: [Selected direction](../specs/2026-11-14-plan-compiler.md).
Execute natively, preserving the existing gate and durable operation semantics.

- [x] Core: typed operation contracts, ordered references, symbolic readership,
  concrete counterexample, runtime obligations and recompiled fixed repairs.
  Write failing tests for unsafe plan, both repairs, mismatched approval,
  unresolved/cyclic references, missing prompt and unauthorized explanations.
- [x] Runtime: immutable plan intake, pinned run reservation, exact tracked inputs,
  broker generation, human review obligation and dispatch-time plan conformance.
  Test blocked preparation, idempotent partial recovery, changed inputs/audience,
  substituted review artifacts and actual approved publication.
- [x] Simulation: finite breadth-first checker using the production verifier,
  declared world/depth bounds and independent invariants. Require a shortest
  counterexample for a broken approval-reuse rule and a clean bounded normal run.
- [x] App/CLI: plan intake/analyze/repair/start/status, plan-first workspace with
  visual typed flow and repair cards; bounded Verification view; compile/check-model
  CLI commands and recorded sample plan JSON.
- [x] Verify Rust gates, actual HTTP and browser compile/repair/review/release flow.
  Save a screenshot and record exact implementation/integration limits.

MCP, live Hydra, Composio and optional Kani are explicitly separate next stages;
the UI and README must not imply these are already qualified.

## Delivery record

The initial compiler/execution slice is implemented. Compiler tests show both
fixed repairs and a concrete Sam/source/path witness. Runtime tests cover all
preparation commit boundaries, input drift, output substitution, authorization
independent of earlier syntax errors and UTF-8-safe long goals.

Observed red/green evidence is retained under evidence/. Final workspace result:
58 passing tests, one explicitly ignored live Hydra test; clean Clippy and format.
The depth-6 model checks 103 nonterminal states and 624 transitions. The deliberate
approval-reuse model fault yields a four-action counterexample. The browser
completed fresh repair, generation, actual-content review and simulated release,
and checked both normal and broken-rule model views.

MCP, real Hydra plan/evidence reads, Composio, provider sharing authorization and
optional Kani are still the next integration/qualification stages. The selected
direction has replaced the inbox as the app's main navigation and product object.

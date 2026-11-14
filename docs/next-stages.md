# Remaining compiler stages

The selected product is a compiler and execution gate for agent plans. The local
first slice implements typed analysis, counterexamples, fixed repairs, recorded
generation, exact human approval, controlled publication and a bounded checker.
These next stages preserve that loop rather than expanding into a generic dashboard.

## TrustGraph-owned MCP interface

The official Rust SDK transport is implemented: an authenticated loopback
Streamable HTTP `/mcp` endpoint and a stdio bridge share the app's single journal
owner. Seven tools provide context, simulation, proposal, analysis, repairs,
preparation and status. No approval or raw publication tool is exposed. Tests
exercise real messages, an actual stdio child and a lost post-commit reply.
See [MCP](mcp.md) and [8090 integration](8090-integration.md) for the runnable
configuration and current limits. Third-party account connection and production
identity remain separate qualification gates.

The current fixture app retains low-level generation/review routes for earlier
workflows. Before requiring plan preflight for every deployed agent action,
restrict that agent surface to plan operations and retire or separately scope the
legacy direct-intake routes. Plan-associated artifacts are already checked on
the lower-level approval/publication paths; unrelated legacy work still uses the
original source-and-approval gate.

Exit: an existing agent can propose a typed plan, inspect its failure, select a
repair and wait for actual human review, with no raw publisher credential path.

## Real Hydra plan/evidence persistence

Start or connect the selected Hydra revision and record its configuration. Pass
the existing real projection test twice over an allocated ID range, force paged
reads, verify causal bookmarks and interrupt writes. Add connected plan, step,
repair, run binding and obligation records to the graph. Load and validate every
referenced record/digest before using graph evidence in a decision.

The local journal remains the single-writer operation authority until a deliberate
authority transition is tested. A completion marker never substitutes for complete
record/reference validation. The existing projection alone does not satisfy this.

Exit: the live plan → inputs → generated result → approval → publication trail
comes from real, validated Hydra reads. Partial or stale graph loads fail closed.

## Composio and model-provider effects

Qualify one dedicated test destination, actual action schema, controlled account,
idempotency behavior and lookup/reconciliation. Store publishing credentials only
behind the Rust dispatcher. Run success, refusal, ambiguous acknowledgment and
restart scenarios through that adapter. An agent must not receive another route
to the same publishing credential.

Composio's [hosted MCP documentation](https://docs.composio.dev/docs/sessions-via-mcp)
states that SDK tool-call modifiers do not run on direct MCP calls. The dispatch
boundary therefore belongs in TrustGraph, rather than depending on an SDK hook.

Before replacing local generation, separately authorize the model provider as a
data recipient and document retention. Capture every supplied input in a fresh
broker-owned context. Never implement clean regeneration by retaining a private
draft, conversation or cached summary. The initial provider enum only accepts
the local fixture; unsupported external providers cannot execute.

Exit: one real action follows actual-content human approval, with a defensible
uncertain-outcome path. Real provider and destination contracts are documented.

## Narrow formal checks and broader synthesis

The finite checker reports a stated abstraction and depth, not whole-system
security. Optional Kani checks should target small pure authorization functions
with explicit bounds after the live loop works. Least-authority plan synthesis
is a later research goal; the current repairs are two fixed, recompiled options.

Related information-flow work includes [Microsoft FIDES](https://github.com/microsoft/fides).
It is relevant prior art, not evidence of TrustGraph's uniqueness. A detailed
comparison with policy compiler/event-history research remains open.

Production identity, storage repair, signed receipts, retention, real process-kill
tests and remote ACL races remain deployment gates. None is established by a
passing finite model or local fixture test.

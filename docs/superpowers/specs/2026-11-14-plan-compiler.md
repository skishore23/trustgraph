# TrustGraph: a compiler for safe agent plans

The user supplied and selected this direction. It supersedes the review inbox as
the primary product. Existing agents propose plans; a human inspects their
information flow and a concrete counterexample, chooses a permitted alternative,
then approves the actual generated result before the controlled effect occurs.
One workspace remains the scope; no billing or tenant management.

## First delivery

Build the smallest useful compiler language: Read, Generate, Approve and Publish.
A plan is an immutable, ordered, typed DAG. References must point backward to
compatible values. The initial language has exactly one penultimate Approve and
one final Publish referencing it. All reads/generations precede that approval.
Multiple approval/resume points are outside this slice. Generate
uses an explicit fresh local fixture context, a fixed launch template and tracked
input artifacts, including the task prompt. Unknown providers are rejected; no
external model receives data in this delivery.

Compilation computes transitive source readership and checks operation contracts.
Approve creates a future obligation, never approval of nonexistent bytes. Publish
must reference an Approve for the same artifact step and audience. A conflict
identifies the first invalid step, one recipient, one restricted source, and the
dependency path. Compilation is advisory until execution checks current evidence.
Only authorized source metadata appears in explanations.

Offer two fixed repairs, each recompiled: change Approve/Publish to an existing
permitted audience; or discard restricted context and generate anew using only
inputs allowed for the intended audience. Clean regeneration does not preserve
the original meaning or promise the same information. Repair never changes ACLs.

Store immutable plans and run reservations in the journal. Running a compiled
plan binds each Read to an exact source artifact and each Generate to a
broker-recorded transformation. Stop at Approve and create the existing human
review request. Plan-associated review cannot substitute another artifact or
destination; dispatch rechecks plan conformance and the existing exact-context
approval gate. Output edits require a new run/plan, not alteration of run bindings.
Partial runs are idempotently resumed only while their pinned input snapshot still
matches. No publication occurs before human approval of actual bytes.

The app opens on Plans, with a typed flow, counterexample and two repair cards.
Reviews and Releases become execution surfaces beneath plans. Retain Sources,
Destinations and Review policy. A bounded Verification view demonstrates a correct
model and an intentionally broken approval-reuse rule without modifying runtime
policy. All local fixtures and simulated effects remain labeled.

## Bounded model

Explore breadth-first every reachable state within a declared depth. The finite
world includes three human readers, public/private source sets, two audiences,
two immutable output versions per source set, approval, missing lineage and one
terminal publication. Fixed time excludes expiry from this model; existing
seeded scenarios continue to test time and I/O faults.

The production verifier decides normal modeled publication. Independent finite
predicates check recipient containment, exact artifact/audience approval, missing
lineage rejection and readership monotonicity. A separate deliberately broken
model rule accepts any prior approval; it must produce a reproducible shortest
counterexample. Report bounds, states, transitions and whether exploration ended
normally. Do not call bounded coverage proof of databases, providers or the app.

## Remaining stages

1. This delivery: compiler, fixed repairs, bounded checker, plan-run enforcement
   and visual plan workspace over the local broker/publisher.
2. Agent interface: TrustGraph-owned MCP propose/analyze/repair/start/status tools.
   Agents receive no human-approval tool or raw publisher credentials. Qualify an
   official SDK/transport rather than improvising an untested protocol.
3. Hydra: store plans/run bindings and load validated evidence from the real
   graph, with deployment contract tests. Existing projection is not sufficient.
4. Composio: one dedicated demo destination behind server-owned dispatch. Confirm
   real schema, authorization, idempotency/lookup and uncertain-outcome behavior.
   Direct hosted MCP access must not bypass the Rust gate. Model-provider sharing
   authorization is a separate effect contract before replacing local generation.
5. Optional narrow Kani checks; larger plan synthesis only after the concrete loop.

The selected product is the end-to-end compile/counterexample/repair/execute
experience. No novelty claim is made against related information-flow or policy
compiler research. Categorical composition describes the checked relationships;
explicit permission predicates remain necessary.

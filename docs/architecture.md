# Architecture

TrustGraph places a deterministic policy decision and a controlled publication
step around agent-generated work. An agent’s ability to read a source does not
mean every department may access what it creates from that source.

## Components

| Component | Responsibility | Implementation |
| --- | --- | --- |
| Agent adapter | Restricted MCP tools and HTTP work intake | `trustgraph-cli/src/mcp.rs`, `trustgraph-live/src/server.rs` |
| Agent input recorder | Fresh Codex turn, selected source bytes, prompt, ordered input manifest and output | `trustgraph-live/src/runner.rs` |
| Policy checker (Rust core) | Traverse dependencies, derive reader sets, verify approval context and produce conflicts | `trustgraph-core` |
| Publishing gate (runtime) | Journal state, recheck authorization, reserve/dispatch/reconcile operations | `trustgraph-runtime` |
| Evidence store | Persist sources, permissions, runs, manifests, outputs, approvals and receipts | HydraDB adapter in `trustgraph-live/src/hydra.rs` |
| Publisher | Verify repository audience, base and exact payload; create branch and draft PR | `trustgraph-live/src/github.rs` |
| Simulator | Evaluate current and proposed policies on isolated hypothetical evidence | Live API, guided story and `trustgraph-sim` |

Paths in this table are relative to `crates/`.

## From task to decision

1. An outer coding agent submits a work order allowed by the operator through MCP or
   HTTP. It receives a work ID immediately and reads status while generation runs.
2. The gateway selects source versions and repository base content, then starts
   a fresh Codex turn with its tools disabled. It records fingerprints and
   typed input occurrences, including the prompt. Existing caller conversations
   and arbitrary caller patches are outside this contract.
3. The generated artifact links to its captured transformation and input
   manifest. Rust walks backward through the graph. It rejects incomplete,
   inconsistent or untrusted lineage rather than assuming the output is public.
4. The output’s permitted readers are the **intersection** of all contributing
   source reader sets. If the directory permits everyone and salaries permit HR,
   only HR-authorized readers remain. This is conservative dependency tracking;
   it does not ask a model whether the wording sounds safe.
5. Rust compares the destination audience with that set. A denial includes the
   restricting source, excluded principal and dependency path. Passing tests and
   human review cannot override missing source access.
6. A fresh repair starts from separately shared sources. It excludes the old
   restricted patch and investigation. An edit of the restricted patch retains
   its dependency; rewriting text cannot erase its history.

## Approval and publication

Approval binds exact output bytes, destination and the policy/source context.
Before release, Rust reconstructs the expected binding and checks reviewer
access, expiry and revocation. Changed bytes, permissions, audience or policy
can invalidate it. The revised artifact needs review; an earlier release remains
a historical fact. This cannot retract copies already downloaded from GitHub.

The live runtime reads back required Hydra evidence, validates content and
relationships, and checks current authorization again. The publisher verifies
the configured private repository audience, unchanged base commit and exact
checked payload. It creates a branch and **draft** PR; it does not merge.

## Why both HydraDB and a local journal?

HydraDB is the connected evidence graph: what was used, how artifacts relate,
what was approved and what happened. It supports reconstruction and inspection.
It is not the authorization evaluator.

The durable local journal is the runtime’s operation log. It persists state and
dispatch intent **before** an GitHub change. Stable operation keys let retries
refer to an existing operation. If a publisher response is lost, the runtime
reconciles the outcome instead of blindly sending again. These concerns are
separate from querying provenance in HydraDB. Replacing the journal would
require equivalent atomic state-transition and recovery semantics.

Task submission itself has no client idempotency-key contract. MCP does not
retry mutations automatically; after a lost reply, inspect existing work before
submitting again. Publication operation idempotency does not imply submission
idempotency.

## Simulation and reproducibility

The no-service guided story fixes the evidence, scenario and evaluation time;
identical comparisons produce identical results and fingerprints. The virtual
runtime campaign also fixes its seed and event schedule and supports replay.

The live simulator supplies current server time, compares active and proposed
policies against the same hypothetical case, and stores evidence in HydraDB.
It does not execute proposed code, activate policy or publish. Decisions are
repeatable for identical explicit evaluator inputs; live timestamps and record
IDs need not be byte-for-byte identical between separate requests.

## Scope

The current broker controls its supplied inputs and the publisher’s credentials.
It is not a global monitor of another agent’s hidden memory or tools. Permission
sets are explicit configured identities, not imported enterprise IAM. Production
use across arbitrary repositories, languages, identity providers or remote
agents requires adapters and deployment work beyond this local integration.

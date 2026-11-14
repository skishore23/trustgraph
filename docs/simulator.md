# Simulation and bounded checking

TrustGraph now has two complementary modes. The finite checker explores a
declared abstraction exhaustively within a depth; the seeded runtime simulator
controls operation order and declared I/O faults. Neither qualifies external
providers or the whole application.

## Finite checker

`trustgraph check-model --depth 6` uses the production Rust verifier to authorize
modeled publication. Independent predicates check literal recipient membership,
exact version/audience approval, complete lineage and restrictive-input
monotonicity. The world has three human readers plus the agent, two document
source sets plus the recorded prompt, two audiences, two immutable versions per
source set and one stored approval. Source ACLs and signer authority are fixed.
Time is fixed; expiry, revocation, external services and concurrency are outside
this abstraction. Other seeded scenarios test those modeled timing/fault cases.

Breadth-first search deduplicates complete model state and explores every
reachable nonterminal state within the configured maximum trace depth (1–8).
Publication outcomes are terminal and counted as transitions. Depth 6 explores
103 states and 624 transitions under the current model version. This is bounded
evidence about the specified world, not unbounded verification of the runtime.

`--broken-rule` introduces a model-only rule that reuses any existing approval.
It finds a shortest four-action failure: generate from company inputs, approve,
generate with restricted input, then publish using the old approval. The actual
production gate rejects the last action. The command exits 1 on a counterexample;
`--output` saves the report. No live application policy is mutated.

## Seeded runtime simulator

The simulator is deliberately small. `trustgraph-sim::run(seed, scenario)` invokes
the actual `trustgraph-runtime::Coordinator` and `trustgraph-core::verify`.
Only the journal and publishing environment are replaced with declared models.
There is no second authorization implementation.

Scheduler version `trustgraph-scheduler-1` orders scheduled actions by virtual
timestamp, then a seeded 64-bit linear congruential rank, then insertion order.
The generator advances with multiplier `6364136223846793005` and increment
`1442695040888963407`, modulo 2^64. These ranks are scheduling inputs, not secrets
or cryptographic randomness. Integer virtual timestamps are seconds.

A saved trace includes schema/scheduler versions, seed, scenario, resolved action
order, timestamps, outcomes, observed publication counts and receipts. Replay
regenerates the scenario and compares the complete trace. Changing scheduler or
scenario semantics requires a trace version change rather than silently treating
older traces as equivalent.

| Scenario | Main invariant |
| --- | --- |
| `demo` | Private input blocks broad delivery; editing invalidates approval; reapproval sends exactly the reviewed bytes |
| `acl-race` | Narrowing source access before publish blocks it; narrowing afterward cannot undo an earlier delivery |
| `revocation-race` | Approval revoked before dispatch cannot authorize delivery |
| `timeout-recovery` | A provider may accept while its response is lost; restart and lookup recover the original operation |
| `journal-recovery` | A committed reservation with a lost response cannot lead to an unrecorded send |
| `expiry` | Approval is invalid at its expiry boundary |
| `audience-change` | Destination changes require a new context approval |
| `policy-change` | Policy revisions invalidate the old context |

Race tests enumerate seeds 0–63, replay each and require both permitted orderings
to be covered. Independently, every observed provider payload is compared with
the stored artifact, destination and hash and its saved decision is replayed.
The campaign command runs eight scenario families across the requested seed
range. This is bounded coverage, not exhaustive model checking.

The memory journal can fail before commit or lose the acknowledgment after
commit, with a configurable number of successful writes before the fault. Runtime
tests exercise failure after sending but before recording the outcome. Disk tests
exercise real reopen, exclusive ownership, blob integrity and torn-tail rejection.

The scheduler does not emulate operating-system scheduling, fsync failure modes,
TCP, Hydra internals, model output quality or external membership propagation.
Real-adapter conformance remains an independent stage.

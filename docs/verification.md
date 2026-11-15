# Verification: three different kinds of evidence

TrustGraph separates ordinary regression tests, simulation, and bounded formal
verification. None is a claim that the whole application is formally verified.

## Kani proof harnesses

`crates/trustgraph-core/src/permissions.rs` separates two allocation-free,
production **sorted-iterator kernels** from their `BTreeSet` adapters:

- `visit_intersection` performs a linear merge and emits common readers.
- `sorted_subset` scans sorted readers to check every destination recipient.
- Public `intersect`/`can_flow` pass real `BTreeSet` iterators into these kernels.
  Intersection buffers common readers in a preallocated Vec and uses bulk
  `collect`, rather than repeated tree insertion. This adapter trades a
  transient result buffer for a single easily verifiable merge loop.
  The scans are O(n + m); they are not benchmarked as equivalent to the standard
  library's adaptive performance on highly skewed set sizes.

The legacy verifier and generic SDK both use those public functions. Kani runs
these exact kernels with stack-array iterators. This avoids symbolically executing
Rust's tree allocation and balancing machinery. It is **not a proof of the full
BTreeSet adapters**. Sorted, unique iteration is an explicit kernel precondition,
guaranteed by `BTreeSet` in production. Regression and property tests compare the
adapters against standard-library intersection/subset on empty, finite and random
sets (random vectors up to 63 `u16` keys are canonicalized into sets).

| Harness | Property |
| --- | --- |
| `intersection_kernel_matches_membership` | Output contains exactly the common readers, with sorted unique output; this includes no widening |
| `subset_kernel_matches_membership` | The scan permits exactly destinations whose members are all readers |
| `composition_is_associative` | Grouping three input restrictions does not change the result |
| `duplicate_or_reordered_inputs_preserve_readers` | Repeated inputs are idempotent; ordering does not change the intersection |
| `recipient_must_be_permitted_by_every_input` | A destination is allowed by the intersection exactly when both input policies allow it |
| `arbitrary_keys_match_set_specification` | Both kernels match membership specifications for arbitrary ordered `u8` keys, not just fixed principal IDs |
| `gate_allows_only_when_every_fact_holds` | `decision::gate` returns `Allowed` exactly when all eight permission facts hold and approval is not required or is live |
| `approval_cannot_turn_a_denial_into_allow` | Varying `approval_required` and `approval_live` never changes a denial gate and never yields `Allowed` |
| `needs_approval_implies_otherwise_allowed` | `NeedsApproval` occurs exactly when the eight permission facts hold, approval is required and not live; the same facts without the requirement give `Allowed` |
| `approval_window_is_exact_and_overflow_free` | `approval_live` equals `issued_at <= now < issued_at + valid_for` computed in 128 bits, and a voided approval is never live |

**Bounds:** the first five `permissions` harnesses (through `recipient_must_be_permitted_by_every_input`) quantify over every subset of `{0, 1, 2}`,
including empty sets (512 combinations for three-set laws), with unwind 5. The
`arbitrary_keys_match_set_specification` uses two arbitrary `u8` arrays, symbolic lengths from 0 through 3,
and assumes only strict ascending order of the active prefixes. It checks exact
intersection, ordered unique output, and subset correctness with unwind 8. The
larger unwind permits merges of disjoint key ranges. No safety, overflow or
unwinding assertions are disabled. An insufficient loop bound fails verification.

**Decision kernel bounds:** the three gate harnesses cover all 1,024 fact combinations; `approval_window_is_exact_and_overflow_free` covers arbitrary u64 inputs; all four are loop-free, so no unwind bound is needed. Their results will be recorded after a CI run.

These are bounded proofs of the production iterator kernels with slice iterators,
not an unbounded theorem for arbitrary sizes, types, graph shapes, or schedules.
The tree iterator, Vec buffering and output collection are outside this proof boundary. It also
does not prove authentication, complete input capture, external publication
atomicity, cryptographic properties, semantic absence of secrets, or correctness
of the full approval verifier.

### Reproduce

On a [Kani-supported platform](https://model-checking.github.io/kani/install-guide.html):

```sh
cargo install --locked kani-verifier --version 0.68.0
cargo kani setup
cargo kani -p trustgraph-core --lib --output-format terse
```

`cargo kani setup` installs the version-matched compiler and CBMC tooling. It does
not change the project's ordinary Rust compiler requirement. The
[`Bounded permission proofs`](../.github/workflows/kani.yml) workflow runs the
same command for relevant changes and can be started manually. The workflow has
read-only repository permissions and a 30-minute timeout.

### Recorded runs

The original full-BTreeSet attempt was **inconclusive**: local runs were stopped
at roughly four minutes and 180 seconds, and the first GitHub Actions run reached
the 30-minute job limit without completing its first harness. A simpler
filter/contains full-BTreeSet variant also timed out locally after 70 seconds.
These are resource-limit outcomes, not counterexamples or proofs. The principal
universe was not reduced to make these attempts pass.

The subsequent kernel refactor removes tree allocation from the proof boundary
rather than pretending it was verified. Kani 0.68.0 / CBMC 6.11.0 /
nightly-2026-08-21 are used. On 2026-11-15 the final code completed **five
successful bounded proofs**:

| Harness | Result | Solver seconds |
| --- | --- | ---: |
| `recipient_must_be_permitted_by_every_input` | Successful, 0 of 584 checks failed | 37.57 |
| `duplicate_or_reordered_inputs_preserve_readers` | Successful, 0 of 516 checks failed | 15.12 |
| `composition_is_associative` | Successful, 0 of 515 checks failed | 27.05 |
| `subset_kernel_matches_membership` | Successful, 0 of 616 checks failed | 13.86 |
| `intersection_kernel_matches_membership` | Successful, 0 of 742 checks failed | 24.85 |
| `arbitrary_keys_match_set_specification` | **Inconclusive: process time budget exhausted** | No completed verdict |

The complete command had a 480-second process budget and exited 124 while checking
the final arbitrary-key harness. Therefore this is **five proved bounded
properties, one uncompleted extension**, not a successful all-harness run. The
five completed checks cover all subsets of the three-principal universe; the
larger arbitrary-key statement remains unproved. The final code also passed 18
core regression/property tests and core clippy with warnings denied.

### CI confirmation

[GitHub Actions run 43680728039](https://github.com/skishore23/trustgraph/actions/runs/43680728039)
verified code commit `a44a2732862e4eb4f332969d77d0314508f74ccd` on
2026-11-15: **all six harnesses successfully verified, zero failures**. The
arbitrary-key extension completed in 156.94 solver seconds with 0 of 597 checks
failed (13 unreachable). The other five completed in 19.20, 7.56, 13.26, 7.26 and
11.99 solver seconds, respectively, in the table's order. This supersedes the
local inconclusive outcome for that extension, without changing the bounds or
expanding the proof to the BTreeSet adapters or the whole application.

Compilation reports potentially unsupported `caller_location` and a foreign
function in the crate. Kani inserts reachable-unsupported checks; those checks
are not disabled. A successful harness means no such unsupported path was
reachable within that harness's assumptions and bounds, **not** that all code in
the crate supports verification. A timeout establishes neither reachability nor
unreachability. Inspect the CI result for the exact commit before claiming that
revision passed. Ordinary `cargo test` success is independent evidence.

## Tests

`cargo test --workspace --locked` runs regression tests, finite reader-algebra
checks, compatibility checks between the old and new APIs, and tests for typed
ports, executor access, approvals, version changes and bounded graph traversal.
These are useful evidence but are not substitutes for Kani's symbolic checks.

## Simulation

The existing `trustgraph-sim` crate executes seeded virtual-time scenarios over
the real legacy coordinator, journal and local publisher. It injects declared
faults and reorders scheduled actions, then checks the recorded effects against
approved evidence. It does not intercept OS threads, packets or database internals.
A simulated schedule is not a proof over every possible distributed execution.

The generic framework lab and the coordinator simulator are related but distinct:
one explores typed dataflow and permission decisions, the other exercises the
existing gateway's recovery, timing and publication behavior. Keeping this
boundary visible prevents a simulator result from overstating the SDK's scope.

### Recorded simulator checks (2026-11-15)

- `simulate --seed 0 --count 64`: 512 scenario runs across 64 seeds passed;
  observed provider payloads matched their exact approved evidence.
- Bounded model exploration at depth 6: 103 states, 624 transitions and six
  publications checked without a reported invariant violation.
- Negative control: the deliberately broken mutant produced the counterexample
  `generate_public → approve → generate_mixed → publish`. The mutant would permit
  an unauthorized audience; the production verifier blocks `RestrictedSource`.
  This validates that the checker can detect that modeled defect, not that it
  detects every possible implementation bug.

These counts describe the tested models and bounds, not production traffic.

Reproduce the coordinator checks locally (no external publication):

```sh
cargo run --locked -p trustgraph-cli -- simulate --seed 0 --count 64
cargo run --locked -p trustgraph-cli -- check-model --depth 6
# Expected to report a counterexample / fail the deliberately broken model:
cargo run --locked -p trustgraph-cli -- check-model --depth 6 --broken-rule
```

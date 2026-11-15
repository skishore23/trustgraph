# Verification: three different kinds of evidence

TrustGraph separates ordinary regression tests, simulation, and bounded formal
verification. None is a claim that the whole application is formally verified.

## Kani proof harnesses

`crates/trustgraph-core/src/permissions.rs` contains `#[cfg(kani)]` harnesses
calling the **production** `intersect` and `can_flow` helpers. The legacy verifier
and generic dataflow SDK both use these helpers. The proofs do not substitute a
second bitmask implementation for the functions actually used by the application.

| Harness | Property |
| --- | --- |
| `intersection_never_widens` | A combined reader set is a subset of both inputs |
| `composition_is_associative` | Grouping three input restrictions does not change the result |
| `duplicate_or_reordered_inputs_preserve_readers` | Repeated inputs are idempotent; ordering does not change the intersection |
| `recipient_must_be_permitted_by_every_input` | A destination is allowed by the intersection exactly when both input policies allow it |

**Bound:** the principal universe is `{0, 1, 2}`. Every membership is a symbolic
Boolean, so each reader set can be any of the eight subsets, including empty.
The three-set harnesses quantify over all 512 combinations within that universe. Loops
are unwound to 16, and unwinding assertions remain enabled: exceeding the bound
fails verification instead of silently excluding executions.

This is a bounded proof of the instantiated Rust helper code and its reachable
standard-library operations, not an unbounded theorem for arbitrary sets, graph
sizes, all Rust types or all workflow schedules. It does not prove authentication,
complete input capture, external publication atomicity, cryptographic properties,
semantic absence of secrets, or correctness of the full approval verifier.

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

### Recorded run

**2026-11-15 local result: harnesses compiled, but no proof verdict completed.**
Installed Kani 0.68.0, CBMC 6.11.0 and nightly-2026-08-21. The all-harness run was
interrupted after roughly four minutes on the recipient-conjunction harness
without a verdict. A subsequent isolated `intersection_never_widens` run with a
180-second process timeout exited 124 while still checking. This is an
**inconclusive resource-bounded attempt**, neither a successful proof nor a
counterexample. No safety or unwinding checks were disabled, and the principal
universe was not reduced to obtain a green result.

Kani also reported possibly unsupported constructs (`caller_location` and foreign
functions) in the compiled code, with verification failing if reachable. Because
checking did not finish, their unreachability is not established by this run.
The CI workflow permits a longer 30-minute job; inspect its result for the exact
commit before making any proof claim. `cargo test` succeeding does not mean Kani
verification succeeded.

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

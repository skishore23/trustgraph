# Verification

TrustGraph has three kinds of evidence: bounded proofs of the core rules,
ordinary tests, and a seeded simulator that drives the real runtime. None of
them is a claim that the whole system is formally verified.

## Kani proof harnesses

The allow / needs-approval / deny ordering and the approval time window live
in one small kernel, `crates/trustgraph-core/src/decision.rs`. Both
`trustgraph_policy::decide` and the runtime engine call it. The reader-set
rules live in `crates/trustgraph-core/src/permissions.rs`, which separates two
allocation-free, sorted-iterator kernels from their `BTreeSet` adapters:

- `visit_intersection` performs a linear merge and emits the common readers.
- `sorted_subset` scans the sorted readers to check every recipient.
- The public `intersect` and `can_flow` pass real `BTreeSet` iterators into
  these kernels. The policy crate and the lineage graph use those public
  functions.

Kani runs the exact kernels with stack-array iterators, which avoids
symbolically executing Rust's tree allocation and balancing. It is **not a
proof of the `BTreeSet` adapters**. Sorted, unique iteration is an explicit
kernel precondition, guaranteed by `BTreeSet` in production. Regression and
property tests compare the adapters with the standard library's intersection
and subset on empty, finite and random sets.

| Harness | Property |
| --- | --- |
| `intersection_kernel_matches_membership` | The output holds exactly the common readers, sorted and unique; nothing is widened |
| `subset_kernel_matches_membership` | The scan permits exactly the recipient sets whose members are all readers |
| `composition_is_associative` | Grouping three input restrictions does not change the result |
| `duplicate_or_reordered_inputs_preserve_readers` | Repeated inputs are idempotent; order does not change the intersection |
| `recipient_must_be_permitted_by_every_input` | A recipient set is allowed by the intersection exactly when both inputs allow it |
| `arbitrary_keys_match_set_specification` | Both kernels match their membership specifications for arbitrary ordered `u8` keys |
| `gate_allows_only_when_every_fact_holds` | `decision::gate` returns `Allowed` exactly when all eight permission facts hold and approval is not required or is live |
| `approval_cannot_turn_a_denial_into_allow` | Varying `approval_required` and `approval_live` never changes a denial and never yields `Allowed` |
| `needs_approval_implies_otherwise_allowed` | `NeedsApproval` occurs exactly when the eight facts hold, approval is required and not live; without the requirement the same facts give `Allowed` |
| `approval_window_is_exact_and_overflow_free` | `approval_live` equals `issued_at <= now < issued_at + valid_for` computed in 128 bits, and a voided approval is never live |

**Bounds.** The first five `permissions` harnesses quantify over every subset
of `{0, 1, 2}`, including empty sets (512 combinations for three-set laws),
with unwind 5. `arbitrary_keys_match_set_specification` uses two arbitrary
`u8` arrays with symbolic lengths 0 to 3, assumes only strict ascending order,
and runs with unwind 8. The three gate harnesses cover all 1,024 fact
combinations and `approval_window_is_exact_and_overflow_free` covers arbitrary
`u64` inputs; all four are loop-free. No safety, overflow or unwinding
assertion is disabled, and an insufficient loop bound fails verification.

These are bounded proofs of the kernels, not a theorem for arbitrary sizes,
types or schedules. They do not cover how callers compute the facts, group
expansion, the journal, authentication, or whether the host reports every
input; tests and the simulator cover those, without proof.

### Recorded CI result

[GitHub Actions run 43787456648](https://github.com/skishore23/trustgraph/actions/runs/43787456648)
verified commit `c48d64cd8078d66de32ed9945a689e4e41cbae37` on 2026-11-15:
**all ten harnesses verified, zero failures**. The table below is from that
run. The kernels and harnesses in `trustgraph-core` are unchanged since.

[Run 43800337348](https://github.com/skishore23/trustgraph/actions/runs/43800337348)
repeated the result on commit `a62adc6`, after the repository was cut down to
the policy product: all ten harnesses verified, zero failures.

| Harness | Result | Verification seconds |
| --- | --- | ---: |
| `decision::gate_allows_only_when_every_fact_holds` | 0 of 13 checks failed | 0.03 |
| `decision::approval_cannot_turn_a_denial_into_allow` | 0 of 14 checks failed | 0.04 |
| `decision::needs_approval_implies_otherwise_allowed` | 0 of 14 checks failed | 0.04 |
| `decision::approval_window_is_exact_and_overflow_free` | 0 of 4 checks failed | 0.07 |
| `permissions::recipient_must_be_permitted_by_every_input` | 0 of 584 checks failed (6 unreachable) | 15.44 |
| `permissions::duplicate_or_reordered_inputs_preserve_readers` | 0 of 516 checks failed (6 unreachable) | 6.19 |
| `permissions::composition_is_associative` | 0 of 515 checks failed (6 unreachable) | 10.63 |
| `permissions::subset_kernel_matches_membership` | 0 of 616 checks failed (18 unreachable) | 5.91 |
| `permissions::intersection_kernel_matches_membership` | 0 of 742 checks failed (18 unreachable) | 9.86 |
| `permissions::arbitrary_keys_match_set_specification` | 0 of 597 checks failed (13 unreachable) | 142.54 |

Compilation reports a potentially unsupported `caller_location` and a foreign
function in the crate. Kani inserts reachable-unsupported checks and they are
not disabled. A successful harness means no unsupported path was reachable
within that harness's assumptions and bounds, not that all code in the crate
supports verification. Check the CI result for an exact commit before claiming
that revision passed.

### Reproduce

On a [Kani-supported platform](https://model-checking.github.io/kani/install-guide.html):

```sh
cargo install --locked kani-verifier --version 0.68.0
cargo kani setup
cargo kani -p trustgraph-core --lib --output-format terse
```

The [`Bounded permission proofs`](../.github/workflows/kani.yml) workflow runs
the same command when `crates/trustgraph-core`, the workspace manifest or the
lock file change, and can be started by hand. It has read-only repository
permissions and a 30-minute timeout.

## Tests

`cargo test --workspace --locked` runs the policy crate's parsing, decision,
explain, diff and example tests; the engine and journal tests (replay, torn
writes, hash chain, approval expiry and revocation); the kernels' regression
and property tests; the lineage graph's tests; the simulator's tests; the CLI
tests; and the `trustgraph serve` tests over real HTTP, MCP and the operator
session. They are evidence, not a substitute for the symbolic checks.

The [Rust workspace CI](../.github/workflows/rust.yml) also checks formatting,
Clippy with warnings denied, every workspace target, the gateway's HTTP/stdio
end-to-end suite, and the executable protected demo. Existing Python CI tests
HTTP/MCP integration and supported framework version ranges; the Kani workflow
continues to verify the bounded kernels separately.

The typed-runtime tests exercise schema mismatch rejection, reader non-widening,
legacy event encodings/hash compatibility, typed ancestry, and replay equivalence.
The protected demo tests denied PR/search forwarding using independent upstream
logs, successful authorized work, conservative classification, and restart and
reconnect behavior. Additional gateway tests cover context migration and approval
invalidation when the resolved upstream account changes. These checks do not
certify arbitrary production MCP adapters or establish model attack-success rates.

## The policy simulator

`crates/trustgraph-sim` drives `Runtime<MemoryJournal>`, the same runtime
`trustgraph serve` runs. Each run is a pure function of the policy text, a
seed and a step count. It makes random interleavings of agent, host and
operator events, clock jumps, policy changes, crashes and torn journal tails,
and after every step checks five invariants:

1. `no_leak`: every decision equals the pure `trustgraph_policy::decide`, and
   every `Allow` names only readers of the output.
2. `approval_scoped`: no `Allow` relies on an approval from before the latest
   activation, or on one that was revoked or expired.
3. `replay_equivalent`: a crash and reopen reach the same state and the same
   next decisions.
4. `narrowing_applies`: after an activation, earlier allowed checks whose
   recipients are no longer readers are denied.
5. `agents_cannot_change_policy`: the policy changes only on an activation
   that returned `Activated`.

**An independent oracle.** Who may read an output is also computed a second
way, straight from the policy text, so neither `decide` nor the engine grades
itself.

**Faults it must catch.** The engine has a `fault-injection` feature, off in
every shipped build (a CLI test checks its dependency graph), with three
deliberately broken variants. The simulator's tests require each to be caught:

- `SkipReaderRefresh` leaves source readers unchanged on activation; caught as
  `narrowing_applies` or `no_leak` when the engine's two deciders disagree.
- `GraphOnlyStaleReaders` also skips `decide`, so a narrowing yields a real
  `Allow`; caught by `narrowing_applies` and by `no_leak` from the independent
  oracle, not the engine.
- `KeepApprovalsAcrossPolicyChange` keeps approvals across an activation;
  caught by `approval_scoped` on a real `Allow`.

**Coverage keys.** `--json` output counts how often each situation occurred:
crashes (clean, lost write, torn tail, without resend), torn approve, revoke
and activate, approval expiry at and before the boundary, revocation, narrowing
and source removal with derived outputs, identical reactivation, approved
allows, explicit recipients and agent-denied sources. A key at 0 means that path was not
exercised in the run.

### Reproduce

```sh
trustgraph policy simulate examples/policies/slack-digest.policy.yaml --seed 0 --count 64
cargo test -p trustgraph-sim --locked
```

```text
64 runs, 13,406 events: 2,461 allow, 3,432 deny, 913 needs_approval — 0 violations
```

The same seed and options always give the same digest; a violation prints the
exact command to replay it.

## Limits

- The proofs are bounded and cover the kernels, not the whole system.
- The simulator shows no violations in the runs it made, not in all possible
  runs. It uses an in-memory journal and does not exercise OS threads, the
  network or a real file system.
- TrustGraph only knows the sources and outputs the host reports, and the
  host must act only on `allow`.

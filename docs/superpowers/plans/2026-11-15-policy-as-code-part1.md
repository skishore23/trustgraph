# Policy-as-code, part 1: policy module, CLI, runtime core and simulator

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A user or an agent can write `trustgraph.policy.yaml`, validate, test, explain, diff and simulate it from the CLI, on a deterministic runtime core that the simulator drives directly.

**Architecture:** A new `trustgraph-policy` crate owns the file format and a pure decision function. A new `trustgraph-engine` crate is a deterministic event-driven state machine over the existing `trustgraph-framework::Graph`, with a journal trait. `trustgraph-sim` gains a seeded simulator that drives that engine and checks five invariants, one of them differentially against the pure decision function. The Composio flow moves to an example crate first.

**Tech Stack:** Rust 1.90, edition 2024, serde, serde_yaml 0.9, serde_json, clap 4, sha2 (all already in `Cargo.lock`).

**Spec:** `docs/superpowers/specs/2026-11-15-policy-as-code-design.md`

This is part 1 of 3. Part 2: `trustgraph serve` (HTTP, MCP, operator page). Part 3: hosted demo page and README rewrite. Each gets its own plan after this one lands.

## Global Constraints

- Rust 1.90, edition 2024, `unsafe_code = "forbid"`; every new crate uses `[lints] workspace = true`.
- Must pass: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked`.
- No new third-party dependencies beyond those already in `Cargo.lock`.
- `trustgraph-policy` and `trustgraph-engine` contain no wall clock, no randomness and no I/O, except `FileJournal`. Use `BTreeMap` / `BTreeSet` only, so output order is stable.
- Decisions are deterministic; no LLM or natural-language rule anywhere.
- Anything not named in the policy is denied.
- An agent-callable event can never change the active policy.
- Do not change the existing Kani harnesses in `crates/trustgraph-core/src/permissions.rs`. New decision logic follows the same pattern: a small allocation-free kernel in `trustgraph-core` with Kani harnesses (Task 2A), called by the policy crate and the engine rather than reimplemented.
- Kani is not installed locally. New harnesses must compile under `cargo check -p trustgraph-core` and are verified by the `Bounded permission proofs` workflow; do not record a proof result in `docs/verification.md` until a CI run has produced it.
- The reference applications (`trustgraph-runtime` PolicySpec, `trustgraph-live` WorkPolicy, story demo) keep their own formats; do not migrate them.
- Do not touch `README.md` beyond fixing links broken by Task 1.
- Commit messages end with a blank line, then a Co-Authored-By trailer naming the model that did the work.

## Review Focus

1. **Hostile or huge policy file** (10 MB, deep nesting, YAML alias expansion): expect one `too_large` or `yaml_syntax` diagnostic within a second, never a hang or panic. → Task 2.
2. **Wrong-typed YAML** (a number or `null` where a name goes, a bare `on` / `no` parsed as a boolean, duplicate keys): expect a diagnostic with the exact path, not a serde message. → Task 2.
3. **Awkward names** (uppercase, spaces, unicode, a name used as both a group and an agent): expect `invalid_name` / `name_collision`, never silent acceptance. → Task 2.
4. **CLI given a missing file, a directory or unreadable file:** expect exit code 2 and one clear line, or a JSON error object with `--json`. → Task 5.
5. **Journal that is corrupt in the middle, written under a different policy, or with time going backwards:** expect `open` to fail closed with a named error; only a torn final line is dropped. → Task 8.

---

### Task 1: Move the Composio / 8090 flow to an example crate

**Files:**
- Create: `examples/integrations/composio-github-issues/Cargo.toml` (package `trustgraph-composio-example`, bin `trustgraph-composio-issues`), `src/main.rs`, `README.md`
- Move (with `git mv`): `crates/trustgraph-cli/src/actions.rs` → `…/src/lib.rs`; `crates/trustgraph-cli/src/actions/` → `…/src/actions/`; `web/actions.{html,css,js}` → `…/web/`; `scripts/{check_composio_github,composio_connect,8090_demo,trustgraph_actions,test_actions,test_8090_demo}.py` and `scripts/test_walkthrough.cjs` → `…/scripts/`; `docs/8090-demo.md`, `docs/8090-issue-actions.md`, `docs/one-hour-issue-demo.md` → `…/docs/`; `examples/8090/` → `…/8090/`; `trustgraph-actions.example.json` → `…/`
- Modify: root `Cargo.toml` (`members = ["crates/*", "examples/integrations/*"]`), `crates/trustgraph-cli/src/main.rs` (remove `Command::Actions` and its arm), `crates/trustgraph-cli/src/lib.rs` (remove `pub mod actions;`), `.gitignore` and `.vercelignore` (ignore `trustgraph-actions*.local.json` at any depth), `README.md:185-189`, `docs/agent-integration.md:7,142`, `docs/vercel.md:80`

**Interfaces:**
- Produces: `trustgraph-composio-issues [--config FILE] [--port 8084] [--agent-port 8085] [--check]`, same flags and defaults the `actions` command had.

- [ ] **Step 1:** Record the baseline: `cargo test --workspace --locked 2>&1 | grep -E "^test result" | awk '{p+=$4} END {print p}'`. Expected: `155`.
- [ ] **Step 2:** Do the moves and create the crate. Its dependencies are the subset of `crates/trustgraph-cli/Cargo.toml` that `actions.rs` uses (it imports `trustgraph_core`, `trustgraph_runtime`, `trustgraph_live::auth`, axum, tokio, serde, reqwest). Fix every `include_str!` path and every script's relative path to the example directory.
- [ ] **Step 3:** `src/main.rs`: a clap parser with the four flags above that calls the moved `check` / `serve` functions exactly as `main.rs:155-166` did.
- [ ] **Step 4:** Write the example `README.md`: what it shows, how to run preview mode, and one sentence stating the pattern: "your tool layer calls TrustGraph before it acts". Link the three moved docs. Fix all moved-doc relative links.
- [ ] **Step 5:** Replace the three stale references in `README.md`, `docs/agent-integration.md` and `docs/vercel.md` with links to the example directory.
- [ ] **Step 6:** Verify. Run `cargo build --workspace` once to refresh `Cargo.lock`, then the three Global Constraints commands; test count is still `155`. Run `cargo run -p trustgraph-composio-example -- --help` (exit 0), every moved `test_*.py` from its new `scripts/` directory, and `node …/scripts/test_walkthrough.cjs`. Run `grep -rn "8090-\|actions\.\(html\|js\|css\)\|trustgraph-actions" README.md docs crates scripts web hosted api` and expect no hits.
- [ ] **Step 7:** Commit: `Move the Composio issue flow to examples/integrations`.

---

### Task 2: `trustgraph-policy` — model, parsing and validation

**Files:**
- Create: `crates/trustgraph-policy/Cargo.toml` (deps: serde, serde_json, `serde_yaml = "0.9"`, `trustgraph-core`), `src/lib.rs`, `src/model.rs`, `src/parse.rs`, `tests/parse.rs`

**Interfaces:**
- Produces:

```rust
pub struct Policy { pub version: u32, pub groups: BTreeMap<String, BTreeSet<String>>,
    pub agents: BTreeSet<String>, pub sources: BTreeMap<String, Source>,
    pub actions: BTreeMap<String, Action>, pub tests: Vec<Case> }
pub struct Source { pub readers: BTreeSet<String>, pub agents: BTreeSet<String> }
pub struct Action { pub agents: BTreeSet<String>, pub audience: BTreeSet<String>,
    pub approval: Option<ApprovalRule> }
pub struct ApprovalRule { pub reviewers: BTreeSet<String>, pub valid_for_seconds: u64 }
pub struct Case { pub name: String, pub output_from: BTreeSet<String>, pub agent: String,
    pub action: String, pub recipients: Option<BTreeSet<String>>, pub expect: Verdict }
pub enum Verdict { Allow, NeedsApproval, Deny }      // serde: "allow" | "needs_approval" | "deny"
pub enum Severity { Error, Warning }
pub struct Diagnostic { pub severity: Severity, pub path: String, pub code: &'static str,
    pub message: String, pub suggestion: Option<String> }
pub struct Parsed { pub policy: Option<Policy>, pub diagnostics: Vec<Diagnostic> }
pub fn parse(yaml: &str) -> Parsed;           // policy is Some only when there are no Errors
impl Policy { pub fn people(&self, names: &BTreeSet<String>) -> BTreeSet<String>;   // expand groups
              pub fn digest(&self) -> String;                                        // sha256 of canonical JSON
              pub fn to_yaml(&self) -> String; }
pub const MAX_BYTES: usize = 262_144;
```

All model types derive `Debug, Clone, PartialEq, Eq, Serialize`.

Rules the builder cannot infer:
- Parse into `serde_yaml::Value` and walk it by hand, so every error is collected in one pass with a path such as `sources.salaries.readers[1]`. Do not rely on `deny_unknown_fields`.
- `version` must be `1` (`unsupported_version`).
- Names (groups, people, agents, sources, actions) match `^[a-z0-9][a-z0-9_.@-]{0,63}$` (`invalid_name`). A name may not be both an agent and a group or person (`name_collision`). A group may not be empty (`empty_group`).
- `readers`, `audience`, `reviewers` entries are a group name or a person who appears in some group; anything else is `unknown_name` with a did-you-mean when a known name is within edit distance 2. `agents` entries must be in top-level `agents`.
- `readers`, `audience`, action `agents` and `reviewers` must be non-empty (`empty_set`). A source's `agents` may be empty (that is a lint warning in Task 3).
- `valid_for` is `<integer><s|m|h|d>`, from 1 second to 30 days inclusive (`invalid_duration`).
- Unknown keys are `unknown_field` with a did-you-mean. Wrong YAML type is `wrong_type` naming the expected type. Input over `MAX_BYTES` is `too_large`, checked before parsing. Syntax errors are `yaml_syntax`.
- Test cases: `output_from` non-empty; `agent`, `action`, sources and `recipients` must exist (`unknown_name`); test names unique (`duplicate_test`).

- [ ] **Step 1: Write failing tests** in `tests/parse.rs`:
  - `spec_example_parses`: the YAML block from the spec's "The policy file" section parses with zero diagnostics; `policy.people(&set(["hr","engineering"])) == set(["nina","sam","ravi"])`; `actions["email_customer"].approval.valid_for_seconds == 1800`.
  - `all_errors_reported_at_once`: a file with a misspelt group in `readers`, an unknown top-level key `sorces` and `valid_for: 30x` yields exactly three Errors with codes `unknown_name`, `unknown_field`, `invalid_duration` and paths `sources.salaries.readers[0]`, `sorces`, `actions.email_customer.approval.valid_for`.
  - `suggests_close_names`: `enginering` → suggestion contains `engineering`.
  - Review Focus 1: `oversized_input_is_rejected_fast` (a 10 MB string → single `too_large`); `alias_bomb_does_not_hang` (nine levels of `&a [*b,*b,…]` anchors → returns an Error diagnostic in under one second, asserted with `Instant`).
  - Review Focus 2: `wrong_types_have_paths` (`readers: 5`, `groups: {hr: null}`, a group key written as bare `on`, a duplicated `sources` key → each an Error with the right path, none containing the text "serde").
  - Review Focus 3: `names_are_strict` (`HR`, `two words`, `münchen` → `invalid_name`; `planner` as both agent and group → `name_collision`).
  - `digest_is_stable_across_key_order`: two files differing only in key order have equal `digest()`.
  - `to_yaml_round_trips`: `parse(p.to_yaml()).policy == Some(p)`.
- [ ] **Step 2:** `cargo test -p trustgraph-policy --test parse` → fails to compile.
- [ ] **Step 3:** Implement `model.rs` and `parse.rs`.
- [ ] **Step 4:** Tests pass; clippy clean.
- [ ] **Step 5:** Commit: `Add trustgraph-policy: strict policy file parsing with path-level diagnostics`.

---

### Task 2A: Decision and approval-window kernels with Kani harnesses

**Files:**
- Create: `crates/trustgraph-core/src/decision.rs`
- Modify: `crates/trustgraph-core/src/lib.rs` (`pub mod decision;`), `docs/verification.md` (add the new harnesses to the harness table with their property and bounds; add no result rows)

**Interfaces:**
- Produces:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Facts { pub action_known: bool, pub agent_known: bool, pub sources_known: bool,
    pub has_sources: bool, pub agent_listed: bool, pub agent_may_use_inputs: bool,
    pub recipients_in_audience: bool, pub audience_in_readers: bool,
    pub approval_required: bool, pub approval_live: bool }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate { UnknownAction, UnknownAgent, UnknownSource, EmptyOutput, AgentNotListed,
    AgentCannotUseSource, RecipientsOutsideAudience, AudienceOutsideReaders, NeedsApproval, Allowed }
pub const fn gate(facts: Facts) -> Gate;          // first failing fact, in the enum's order
impl Gate { pub const fn is_denial(self) -> bool; pub const fn code(self) -> &'static str; }
/// True exactly when issued_at <= now < issued_at + valid_for, with no overflow, and not voided.
pub const fn approval_live(issued_at: u64, valid_for: u64, now: u64, voided: bool) -> bool;
```

`Gate::code` returns the snake_case codes Task 3 lists (`unknown_action` … `audience_outside_readers`, `needs_approval`, `allowed`). No allocation, no panics, no arithmetic that can overflow (`now - issued_at < valid_for` after checking `issued_at <= now`).

- [ ] **Step 1: Write failing unit tests** in the module (`#[cfg(test)]`), each looping over all 1 024 `Facts` values:
  - `allowed_requires_every_fact`: `gate(f) == Allowed` exactly when the eight permission facts are all true and (`!approval_required || approval_live`).
  - `approval_never_overrides_a_denial`: for every `f`, if `gate(f).is_denial()` then `gate` with `approval_live` and `approval_required` set to any of the four combinations is the same denial.
  - `needs_approval_only_when_otherwise_allowed`: `gate(f) == NeedsApproval` exactly when the eight facts hold, approval is required and not live.
  - `first_failing_fact_wins`: with `action_known == false` the gate is `UnknownAction` whatever else is set, and so on down the order.
  - `approval_window_edges`: live at `issued_at` and at `issued_at + valid_for - 1`; not live at `issued_at - 1`, at `issued_at + valid_for`, when voided, or when `valid_for == 0`; `approval_live(u64::MAX, u64::MAX, u64::MAX, false)` is true and does not panic.
- [ ] **Step 2:** Run `cargo test -p trustgraph-core decision`; expect compile failure.
- [ ] **Step 3:** Implement `gate`, `Gate::is_denial`, `Gate::code`, `approval_live`.
- [ ] **Step 4:** Add `#[cfg(kani)] mod proofs` with these harnesses, using `kani::any()` for every field and no `kani::assume` beyond what is stated:
  - `gate_allows_only_when_every_fact_holds` (the first unit test as a symbolic property).
  - `approval_cannot_turn_a_denial_into_allow` (the second).
  - `needs_approval_implies_otherwise_allowed` (the third).
  - `approval_window_is_exact_and_overflow_free`: for arbitrary `u64` inputs, `approval_live(i, v, n, false) == ((i as u128) <= (n as u128) && (n as u128) < (i as u128) + (v as u128))`, and voided is always false.
- [ ] **Step 5:** `cargo test -p trustgraph-core` passes; `cargo check -p trustgraph-core --all-targets` and clippy are clean (the `cfg(kani)` code is covered by the workspace `check-cfg`). Add the four harnesses to the table in `docs/verification.md` with property and bounds ("all 1 024 fact combinations; arbitrary u64 times") and a line saying results are recorded after the CI run.
- [ ] **Step 6:** Commit: `Add proof-friendly decision and approval-window kernels with Kani harnesses`.

---

### Task 3: Pure decisions, inline tests and lints

**Files:**
- Create: `crates/trustgraph-policy/src/decide.rs`, `src/lint.rs`, `tests/decide.rs`

**Interfaces:**
- Consumes: Task 2 types; `trustgraph_core::permissions::{intersect, can_flow}`; `trustgraph_core::decision::{Facts, Gate, gate}`.
- Produces:

```rust
pub struct Query { pub output_from: BTreeSet<String>, pub agent: String, pub action: String,
    pub recipients: Option<BTreeSet<String>> }          // recipients are people or groups
pub struct Blocking { pub source: String, pub readers: BTreeSet<String>,      // names as written
    pub excluded: BTreeSet<String> }                                           // people left out
pub struct Outcome { pub verdict: Verdict, pub code: &'static str, pub reason: String,
    pub blocking: Vec<Blocking>, pub readers: BTreeSet<String> }               // people who may read the output
pub fn decide(policy: &Policy, query: &Query) -> Outcome;
pub struct TestResult { pub name: String, pub expect: Verdict, pub outcome: Outcome, pub pass: bool }
pub fn run_tests(policy: &Policy) -> Vec<TestResult>;
pub fn lint(policy: &Policy) -> Vec<Diagnostic>;        // Warnings, except reviewers_cannot_read which is an Error
```

`decide` computes the ten `Facts`, calls `gate`, and takes `verdict` and `code` from the returned `Gate`; it must not contain its own ordering logic (pass `approval_live: false`, since a pure query has no approval). Decision order and `code` values (first match wins): `unknown_action`, `unknown_agent`, `unknown_source`, `empty_output` (no sources), `agent_not_listed_for_action`, `agent_cannot_use_source`, `recipients_outside_audience`, `audience_outside_readers`, then `needs_approval` (rule has approval) or `allowed`. Reader intersection and the subset check must call the two `trustgraph_core::permissions` kernels on expanded people; do not reimplement them. `audience_outside_readers` fills `blocking` with one entry per source that excludes at least one recipient, and its `reason` reads: ``denied: `salaries` is readable by hr only; engineering (ravi, sam) is not included``.

Lints (codes): `no_tests`, `source_unusable` (no agent may use it), `action_unsatisfiable` (no single source could ever satisfy its audience), `reviewers_cannot_read` (no reviewer is a reader of any source that could flow to the action; **Error** severity, per the spec — such a rule can never be approved), `unused_group`, `unused_agent`. The spec's "approver who is also the only agent" lint is dropped: Task 2's `name_collision` rule already makes it impossible.

`Outcome`, `Blocking`, `TestResult`, and Task 4's `Change` and `Diff` also derive `Debug, Clone, PartialEq, Eq, Serialize`.

- [ ] **Step 1: Write failing tests** in `tests/decide.rs`, all against the spec example:
  - `salary_summary_is_denied_for_slack`: sources `{salaries, roadmap}`, agent `planner`, action `post_to_slack` → `Deny`, code `audience_outside_readers`, `blocking == [salaries]` with `excluded == {ravi, sam}`, reason equals the string above.
  - `roadmap_summary_is_allowed`: `{roadmap}` → `Allow`, `readers == {nina, ravi, sam}`.
  - `price_sheet_email_needs_approval`: `{price_sheet}`, `mailer`, `email_customer` → `NeedsApproval`.
  - `wrong_agent_is_denied`: `mailer` attempting `post_to_slack` → code `agent_not_listed_for_action`; `planner` with `{price_sheet}` → `agent_cannot_use_source`.
  - `unknowns_are_denied`: unknown action, agent and source each give their code.
  - `explicit_recipients_must_be_in_audience`: recipients `{nina}` for `post_to_slack` → `recipients_outside_audience`; recipients `{sam}` → `Allow`.
  - `person_in_two_groups`: add `nina` to `engineering`; salary summary with recipients `{nina}` → `Allow`.
  - `inline_tests_run`: `run_tests` on the spec example returns two results, both `pass`; flipping one `expect` makes exactly that one fail.
  - `lints_fire`: one minimal policy per lint code yields that code; the spec example yields none.
- [ ] **Step 2:** Run; expect compile failure.
- [ ] **Step 3:** Implement `decide.rs` and `lint.rs`.
- [ ] **Step 4:** Tests pass.
- [ ] **Step 5:** Commit: `Add pure policy decisions, inline tests and lints`.

---

### Task 4: Explain and diff

**Files:**
- Create: `crates/trustgraph-policy/src/explain.rs`, `src/diff.rs`, `tests/explain_diff.rs`

**Interfaces:**
- Produces:

```rust
pub fn explain(policy: &Policy) -> String;                       // plain text, stable order
pub enum ChangeKind { Widens, Narrows, Neutral }
pub struct Change { pub kind: ChangeKind, pub subject: String,   // "source salaries", "action post_to_slack"
    pub gained: BTreeSet<String>, pub lost: BTreeSet<String>,    // people
    pub summary: String }
pub struct Diff { pub changes: Vec<Change> }
impl Diff { pub fn widens(&self) -> bool; pub fn is_empty(&self) -> bool; }
pub fn diff(old: &Policy, new: &Policy) -> Diff;
```

`diff` compares on expanded people, so a group membership change shows up on every source and action that names the group. `Widens`: a source gains a reader or an agent; an action gains an audience member or an agent; an approval rule is removed, gains a reviewer or gets a longer `valid_for`; a source or action is added. The reverse is `Narrows`. Test-only edits are `Neutral`.

`explain` output: one line per source — ``salaries: output can reach hr (nina). Usable by agent planner.`` — and one per action — ``email_customer: mailer may send to customers (acme) after approval by sales (leo), valid 30m.``

- [ ] **Step 1: Write failing tests:** `explain_spec_example` asserts the two lines above appear verbatim; `adding_a_reader_widens` (add `engineering` to `salaries.readers` → one `Widens` change, `gained == {ravi, sam}`, `diff.widens()`); `removing_approval_widens`; `group_membership_change_propagates` (add `ravi` to `hr` → `Widens` on `source salaries`); `narrowing_does_not_widen`; `identical_is_empty`.
- [ ] **Step 2–4:** Fail, implement, pass.
- [ ] **Step 5:** Commit: `Add plain-English policy explain and access diff`.

---

### Task 5: Schema, starter file and the `trustgraph policy` commands

**Files:**
- Create: `crates/trustgraph-policy/src/schema.rs`, `crates/trustgraph-policy/starter.policy.yaml`, `crates/trustgraph-cli/src/policy_cmd.rs`, `crates/trustgraph-cli/tests/policy_cli.rs`
- Modify: `crates/trustgraph-cli/Cargo.toml` (add `trustgraph-policy`), `crates/trustgraph-cli/src/main.rs` (add `Command::Policy { #[command(subcommand)] command: PolicyCommand }`; change `about` to `"Policy-as-code for what agents produce"`), `crates/trustgraph-cli/src/lib.rs`

**Interfaces:**
- Produces: `trustgraph_policy::schema() -> serde_json::Value` (JSON Schema draft 2020-12, hand-written, `additionalProperties: false` throughout); `trustgraph_policy::STARTER: &str`.
- CLI, each with `--json`:

| Command | Human output | JSON output | Exit |
| --- | --- | --- | --- |
| `policy init [--path trustgraph.policy.yaml] [--force]` | "Wrote …" | `{"ok":true,"path":…}` | 2 if the file exists without `--force` |
| `policy schema` | the schema | the schema | 0 |
| `policy check FILE` | one line per diagnostic: `error sources.salaries.readers[0] unknown_name: … (did you mean …?)` | `{"ok":bool,"diagnostics":[…]}` | 1 on any Error |
| `policy test FILE` | `PASS name` / `FAIL name: expected deny, got allow — reason` | `{"ok":bool,"results":[…]}` | 1 on any failure or parse Error |
| `policy explain FILE` | text | `{"ok":true,"text":…}` | 1 on parse Error |
| `policy diff OLD NEW` | one line per change, widening lines prefixed `WIDENS` | `{"ok":true,"widens":bool,"changes":[…]}` | 0; 1 on parse Error |

`check` output includes lints (Warnings do not affect the exit code; a lint Error does). I/O failures exit 2 with `error: cannot read FILE: <os error>`, or `{"ok":false,"error":{"code":"io","message":…}}` under `--json`.

- [ ] **Step 1: Write failing tests** in `tests/policy_cli.rs`, running the built binary with `env!("CARGO_BIN_EXE_trustgraph")` in a `tempfile::tempdir()`:
  - `init_then_check_and_test_pass`: `init`, then `check` and `test` exit 0, and `check --json` has an empty `diagnostics` array.
  - `init_refuses_to_overwrite`: second `init` exits 2; with `--force` exits 0.
  - `check_json_reports_path_code_suggestion`: a file with `enginering` exits 1 and the JSON has `path`, `code == "unknown_name"`, a `suggestion`.
  - `failing_inline_test_exits_1`.
  - `diff_marks_widening`: JSON `widens == true`, human output contains `WIDENS`.
  - `schema_validates_shape`: output parses as JSON and `["properties"]["sources"]` exists.
  - Review Focus 4: `missing_file_exits_2`, `directory_exits_2`, and `--json` variants print the `io` error object on stdout.
  - In `crates/trustgraph-policy/tests/parse.rs` add `starter_is_clean`: `parse(STARTER)` has zero diagnostics, `lint` is empty and every inline test passes.
- [ ] **Step 2–4:** Fail, implement, pass. The starter is a commented version of the spec example.
- [ ] **Step 5:** Commit: `Add trustgraph policy init, schema, check, test, explain and diff`.

---

### Task 6: Worked examples and the agent guide

**Files:**
- Create: `examples/policies/{slack-digest,customer-email,pull-request-bot,support-replies,multi-agent-research}.policy.yaml`, `docs/agents/writing-policies.md`, `llms.txt`, `crates/trustgraph-policy/tests/examples.rs`

**Interfaces:**
- Consumes: `parse`, `lint`, `run_tests`.

Each example: 2–4 sources of mixed sensitivity, 2–3 actions of which at least one has an approval rule, and at least three inline tests covering `allow`, `deny` and `needs_approval`. A header comment states the scenario in two sentences.

`docs/agents/writing-policies.md` is written for an agent reader: the format in one annotated example; the loop `init → edit → check --json → test --json → explain → diff → hand to a human`; the diagnostic codes and how to fix each; the rule that agents propose and humans activate; a recommendation to protect the policy file with code review. `llms.txt` at the repo root links the guide, the schema command and the examples directory.

- [ ] **Step 1: Write the failing test** `every_example_is_clean`: for each `examples/policies/*.policy.yaml`, zero diagnostics, zero lints, ≥3 inline tests, all passing, with `allow`, `deny` and `needs_approval` each expected at least once. Assert exactly five files.
- [ ] **Step 2–4:** Fail, write the five files and the guide, pass. Every command shown in the guide must run as written against `examples/policies/slack-digest.policy.yaml`; run each one.
- [ ] **Step 5:** Commit: `Add worked policy examples and the agent authoring guide`.

---

### Task 7: `trustgraph-engine` — the deterministic runtime core

**Files:**
- Create: `crates/trustgraph-engine/Cargo.toml` (deps: serde, serde_json, thiserror, `trustgraph-policy`, `trustgraph-framework`, `trustgraph-core`), `src/lib.rs`, `src/engine.rs`, `tests/engine.rs`
- Modify: `crates/trustgraph-framework/src/lib.rs` — add `pub fn sources(&self, artifact: &ArtifactId) -> Result<BTreeMap<ArtifactId, SourcePermission>>` (the second half of the private `lineage`), with one test in `crates/trustgraph-framework/tests/dataflow.rs`.

**Interfaces:**
- Consumes: `Policy`, `parse`, `decide`, `diff`, `explain`, `run_tests`, `Outcome`, `Verdict`; `trustgraph_framework::Graph`.
- Produces:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Event {
    Genesis { policy: String },                                  // system only: first record, or operator restart with a new file
    RegisterSource { source: String, sha256: String },
    RecordOutput { agent: String, step: String, inputs: Vec<String>, sha256: String },
    Check { agent: String, artifact: String, action: String, recipients: Option<BTreeSet<String>> },
    Approve { decision: String, reviewer: String },
    Revoke { decision: String, reviewer: String },
    ProposePolicy { agent: Option<String>, policy: String },
    ActivatePolicy { proposal: String, operator: String },
}
impl Event { pub fn agent_callable(&self) -> bool; }             // true only for RegisterSource, RecordOutput, Check, ProposePolicy

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Reply {
    Artifact { id: String },
    Decision { id: String, verdict: Verdict, code: String, reason: String, blocking: Vec<Blocking>, expires_at: Option<u64> },
    Approved { decision: String, expires_at: u64 },
    Revoked { decision: String },
    Proposed { id: String, widens: bool, changes: Vec<Change>, explanation: String },
    Activated { revision: u64, digest: String },
    Rejected { code: String, message: String },
}
pub struct Engine { /* private */ }
impl Engine {
    pub fn new() -> Self;                                         // no policy until Genesis; every other event is Rejected{code:"no_policy"}
    pub fn apply(&mut self, at: u64, event: &Event) -> Reply;     // at = seconds; never panics
    pub fn policy(&self) -> Option<&Policy>;
    pub fn policy_revision(&self) -> u64;
    pub fn state_digest(&self) -> String;
    pub fn lineage(&self, artifact: &str) -> Option<BTreeSet<String>>;   // source names
    pub fn with_fault(self, fault: Fault) -> Self;                // #[doc(hidden)], used by the simulator's mutation tests
}
#[doc(hidden)] pub enum Fault { None, SkipReaderRefresh, KeepApprovalsAcrossPolicyChange }
```

Semantics the builder cannot infer:
- `sha256` is 64 lowercase hex or the event is `Rejected{code:"invalid_digest"}`. The engine stores the digest string as the artifact's bytes; it never sees content.
- `RegisterSource`: the name must be in the active policy (`unknown_source`). The artifact id is stable for the same name and digest.
- `RecordOutput`: unknown agent, unknown input artifact, or an input the agent may not use is `Rejected` with codes `unknown_agent`, `unknown_artifact`, `agent_cannot_use_source`. 1–64 inputs. Map to the framework by registering one operation per `(step, input count)` with ports `in0…`, schema `any`, executor = the agent.
- `Check` returns `Reply::Decision`. Its `verdict`, `code`, `reason` and `blocking` must equal `decide(policy, query)` for the artifact's source names — call `decide` for the reason and blocking, and call `Graph::check_action` for the verdict; if the two verdicts disagree, return `Deny` with code `engine_mismatch`. The decision `id` is the framework context digest for `Allow` / `NeedsApproval`, and a hash of the query for `Deny`. A `NeedsApproval` decision is remembered so it can be approved.
- Approval expiry, revocation and voiding go through `trustgraph_core::decision::approval_live`; the engine keeps `issued_at`, `valid_for` and a voided flag per approval and does not compare times itself.
- `Approve`: `reviewer` must be in the rule's reviewers, a reader of the output and not the agent (`reviewer_not_allowed`); unknown decision is `unknown_decision`. A later `Check` of the same query returns `Allow` with `expires_at` until the approval expires, is revoked or is voided.
- `ProposePolicy`: parse errors, lint Errors or failing inline tests → `Rejected{code:"invalid_policy"}`. Otherwise store it; the id is the policy digest. Never changes the active policy.
- `ActivatePolicy`: unknown proposal → `unknown_proposal`. On success: revision + 1; refresh every registered source's readers from the new policy by source name (a source no longer in the policy gets an empty reader set); reset every action rule (a removed action gets an empty actor set); drop every approval and remembered decision.
- `Genesis` behaves like propose + activate in one step and is the only event accepted before a policy exists.
- `at` lower than the previous event's `at` → `Rejected{code:"clock_regressed"}` with no state change.
- `state_digest` covers the policy digest, revision, artifacts, lineage, approvals with expiry, and proposals.

- [ ] **Step 1: Write failing tests** in `tests/engine.rs` (helper `sha(n)` returns a valid digest; policy = spec example):
  - `end_to_end_matches_the_spec_example`: genesis, register `salaries` and `roadmap`, `planner` records an output from both, check `post_to_slack` → `Deny` with code `audience_outside_readers` and blocking source `salaries`; an output from `roadmap` only → `Allow`.
  - `approval_flow`: `mailer` output from `price_sheet`, check `email_customer` → `NeedsApproval`; `Approve` by `leo` → `Approved{expires_at: at+1800}`; check → `Allow`; check at `at+1800` → `NeedsApproval`.
  - `reviewer_rules`: approve by `nina` → `reviewer_not_allowed`; approve by `mailer` → `reviewer_not_allowed`.
  - `revoke_voids`: after `Revoke`, check → `NeedsApproval`.
  - `narrowing_applies_to_existing_outputs`: output from `roadmap` is `Allow`; propose and activate a policy where `roadmap.readers` is `[hr]`; the same check → `Deny`.
  - `removed_source_denies`; `policy_change_voids_approvals`.
  - `proposal_never_changes_active_policy`: `policy_revision()` and `policy().digest()` are unchanged after `ProposePolicy`; an invalid or test-failing proposal is `Rejected`.
  - `only_genesis_before_policy`; `clock_regression_is_rejected_without_state_change` (digest equal before and after).
  - `agent_callable_classification`: exactly four variants return true.
  - `replay_is_deterministic`: applying the same `(at, event)` list to two engines gives equal reply lists and equal `state_digest()`.
  - `garbage_never_panics`: every string field set to `""`, a 10 000-character string and `"€"` in turn → always `Rejected`.
- [ ] **Step 2–4:** Fail, implement, pass.
- [ ] **Step 5:** Commit: `Add trustgraph-engine: deterministic policy runtime core`.

---

### Task 8: Journal and `Runtime`

**Files:**
- Create: `crates/trustgraph-engine/src/journal.rs`, `src/runtime.rs`, `tests/journal.rs`
- Modify: `crates/trustgraph-engine/Cargo.toml` (add `sha2`, `fs2`; dev-dep `tempfile`)

**Interfaces:**
- Produces:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record { pub seq: u64, pub at: u64, pub event: Event, pub prev: String, pub hash: String }
pub trait Journal { fn load(&mut self) -> Result<Vec<Record>, JournalError>;
                    fn append(&mut self, record: &Record) -> Result<(), JournalError>; }
pub enum JournalError { Io(String), Corrupt { seq: u64, reason: String }, Locked }
pub struct FileJournal;     impl FileJournal { pub fn open(dir: &Path) -> Result<Self, JournalError>; }
pub struct MemoryJournal;   impl MemoryJournal { pub fn new() -> Self; pub fn fail_next_append(&mut self);
                                                 pub fn tear_tail(&mut self); pub fn records(&self) -> &[Record]; }
pub struct Runtime<J: Journal> { /* private */ }
impl<J: Journal> Runtime<J> {
    pub fn open(journal: J, policy_yaml: &str, at: u64) -> Result<Self, OpenError>;
    pub fn handle(&mut self, at: u64, event: Event) -> Result<Reply, JournalError>;
    pub fn engine(&self) -> &Engine;
    pub fn into_journal(self) -> J;
}
pub enum OpenError { Journal(JournalError), InvalidPolicy(Vec<Diagnostic>), Replay { seq: u64, reason: String } }
```

Rules:
- `FileJournal`: one file `journal.jsonl` in `dir`, one JSON `Record` per line; `hash = sha256(prev ‖ seq ‖ at ‖ canonical event JSON)`; an exclusive `fs2` lock held for the journal's lifetime (`Locked` if taken); `append` writes the line then `sync_data`. On `load`, a final line that is incomplete or fails to parse is dropped and the file truncated to the last good record; any earlier bad line, sequence gap or hash-chain break is `Corrupt`.
- `Runtime::handle` appends first, then applies. If the append fails, nothing is applied and the error is returned. Events for which the engine would return `Rejected` are still journaled, so replay is exact.
- `Runtime::open`: load and replay. Empty journal → append `Genesis` with `policy_yaml`. Non-empty journal whose active policy digest differs from `policy_yaml`'s → append a new `Genesis` (the operator restarted with a new file). A replayed record with `at` lower than its predecessor → `OpenError::Replay`.

- [ ] **Step 1: Write failing tests** in `tests/journal.rs`:
  - `reopen_reaches_the_same_state`: run ten events through a `FileJournal` runtime, drop it, reopen with the same policy → equal `state_digest()`, and no second `Genesis` in the file.
  - `new_policy_file_appends_genesis`: reopen with a narrowed policy → revision 2 and an earlier `Allow` now denies.
  - `failed_append_applies_nothing`: `MemoryJournal::fail_next_append`, then `handle` returns `Err` and the digest is unchanged.
  - `second_open_is_locked`.
  - Review Focus 5: `torn_tail_is_dropped` (append half a JSON line to the file → opens, state equals the state before that line); `corrupt_middle_fails_closed` (flip one character in line 3 of 6 → `OpenError::Journal(Corrupt{seq:3,..})`); `reordered_time_fails_closed` (hand-write a valid-hash record with a lower `at` → `OpenError::Replay`); `invalid_policy_file_fails_closed` → `OpenError::InvalidPolicy`.
- [ ] **Step 2–4:** Fail, implement, pass.
- [ ] **Step 5:** Commit: `Add hash-chained journal and Runtime with exact replay`.

---

### Task 9: The simulator drives the runtime

**Files:**
- Create: `crates/trustgraph-sim/src/policy_sim.rs`, `crates/trustgraph-sim/tests/policy_sim.rs`
- Modify: `crates/trustgraph-sim/Cargo.toml` (add `trustgraph-policy`, `trustgraph-engine`), `crates/trustgraph-sim/src/lib.rs` (`pub mod policy_sim;`)

**Interfaces:**
- Consumes: `Runtime<MemoryJournal>`, `Engine`, `Event`, `Reply`, `Fault`, `decide`, `Query`, `Policy`.
- Produces:

```rust
pub struct Violation { pub invariant: &'static str, pub step: u32, pub detail: String }
pub struct Run { pub seed: u64, pub steps: u32, pub events: Vec<(u64, Event)>, pub replies: Vec<Reply>,
    pub violations: Vec<Violation>, pub state_digest: String, pub crashes: u32, pub policy_changes: u32 }
pub fn run(policy_yaml: &str, seed: u64, steps: u32) -> Result<Run, Vec<Diagnostic>>;
pub struct Campaign { pub first_seed: u64, pub count: u64, pub steps: u32, pub events: u64,
    pub decisions: BTreeMap<String, u64>, pub violations: Vec<(u64, Violation)>, pub digest: String }
pub fn campaign(policy_yaml: &str, first_seed: u64, count: u64, steps: u32) -> Result<Campaign, Vec<Diagnostic>>;
#[doc(hidden)] pub fn run_with_fault(policy_yaml: &str, seed: u64, steps: u32, fault: Fault) -> Result<Run, Vec<Diagnostic>>;
```

Generator: a splitmix64 PRNG seeded from `seed` (no new dependency). Start by registering every source. Then each step picks, weighted: record an output (random agent, 1–3 random existing artifacts, including ones the agent may not use), check (random agent, artifact, action, sometimes explicit recipients), approve (random person, including non-reviewers), revoke, advance the clock by 0 to twice the longest `valid_for`, propose a mutated policy and sometimes activate it, crash and reopen from the journal, tear the journal tail then reopen. Policy mutations: remove one reader from a source, add one, remove a source, remove an action's approval, shorten `valid_for`, add a person to a group. Agents only ever send `agent_callable` events.

Invariants, checked after every step, with these exact names:
1. `no_leak` — for every `Allow`, every recipient is in `decide`'s `readers` for that artifact's sources under the policy active at that step, and the reply's verdict and code equal `decide`'s.
2. `approval_scoped` — no `Allow` relies on an approval issued before the most recent activation, revocation or expiry that applies to it.
3. `replay_equivalent` — after every crash, the reopened `state_digest()` equals the digest before the crash, and re-sending the last five checks gives the same replies.
4. `narrowing_applies` — after an activation, every remembered earlier `Allow` whose recipients are no longer readers now returns `Deny`.
5. `agents_cannot_change_policy` — `policy_revision()` and the active policy digest change only on a step whose event is `ActivatePolicy` or `Genesis`.

- [ ] **Step 1: Write failing tests** in `tests/policy_sim.rs`:
  - `examples_hold_under_simulation`: for each file in `examples/policies/`, `campaign(yaml, 0, 64, 200)` has zero violations, `events > 10_000`, and its `decisions` map contains all of `allow`, `deny` and `needs_approval`.
  - `runs_exercise_faults`: across those campaigns, total `crashes > 0` and `policy_changes > 0`.
  - `same_seed_same_run`: two `run(yaml, 7, 200)` calls have equal `state_digest` and equal `events`.
  - `different_seeds_differ`: seeds 1 and 2 have different `events`.
  - `skip_reader_refresh_is_caught`: with `Fault::SkipReaderRefresh`, some seed in `0..64` reports `narrowing_applies` or `no_leak`.
  - `kept_approvals_are_caught`: with `Fault::KeepApprovalsAcrossPolicyChange`, some seed in `0..64` reports `approval_scoped`.
  - `invalid_policy_is_an_error`: `run("version: 2", 0, 10)` is `Err`.
- [ ] **Step 2–4:** Fail, implement, pass. If a real violation appears on an example policy, stop and fix the engine, not the simulator; record the seed in the commit message.
- [ ] **Step 5:** Commit: `Drive the policy runtime with a seeded simulator and five invariants`.

---

### Task 10: `trustgraph policy simulate`

**Files:**
- Modify: `crates/trustgraph-cli/src/policy_cmd.rs`, `crates/trustgraph-cli/tests/policy_cli.rs`, `docs/agents/writing-policies.md` (add simulate to the loop), `docs/development.md` (add the three `policy` check commands under local checks)

**Interfaces:**
- Consumes: `trustgraph_sim::policy_sim::{campaign, run}`.
- Produces: `trustgraph policy simulate FILE [--seed 0] [--count 64] [--steps 200] [--json]`. Human output: `64 runs, 12 800 events: 3 912 allow, 7 204 deny, 1 684 needs_approval — 0 violations`, then for each violation `VIOLATION no_leak seed=17 step=93: <detail>` and `replay: trustgraph policy simulate FILE --seed 17 --count 1`. JSON: the `Campaign` plus `"ok": bool`. Exit 1 on any violation or parse Error. Limits: `count` 1–10 000, `steps` 1–5 000, else exit 2.

- [ ] **Step 1: Write failing tests:** `simulate_passes_on_an_example` (exit 0, JSON `ok == true`, `violations == []`); `simulate_is_reproducible` (two runs with `--seed 3 --count 4` have equal `digest`); `simulate_rejects_bad_limits` (`--count 0` exits 2); `simulate_reports_parse_errors` (invalid file exits 1 with diagnostics).
- [ ] **Step 2–4:** Fail, implement, pass.
- [ ] **Step 5:** Run all three Global Constraints commands and every script test once more. Commit: `Add trustgraph policy simulate`.

# TrustGraph: a reviewable payment fix

The business question is **“Can we release this exact agent-generated change?”**

Maya owns releases for a bank's payment service. Zero-value card authorizations
are being rejected, interrupting customer onboarding. She asks a coding agent for
a small fix, while keeping the release boundary narrow: one file, passing tests,
complete input lineage and her approval of the exact deliverable.

The bank is the business framing. The working software is the dedicated private
`skishore23/trustgraph-agent-workbench` repository. It has a real Rust bug, actual
Codex generation, executed tests and live HydraDB evidence. It contains no bank
customer data. The default publication action is simulated.

## The five-minute story

| Scene | Screen | What to do | What the audience should understand |
| --- | --- | --- | --- |
| 1 · Own the boundary | Policy | Show `src/lib.rs`, one changed file, passing tests, 30-minute approval | The release owner decides the allowed scope before an agent acts. |
| 2 · Experiment safely | Simulator | Compare code + docs against a candidate allowing two files | The same proposal can be blocked by the active policy and permitted by a candidate, without activating it. |
| 3 · Approval has a scope | Simulator | Choose Patch changed, then compare again | Allowing a file does not authorize reusing approval for changed content. |
| 4 · Run the agent | Agent | Run Codex; inspect the patch, three executed tests and actual activity | The real workflow captures inputs and checks the actual returned code. |
| 5 · Release with evidence | Agent | Approve exact change, then run publication gate | Current approval and live HydraDB evidence are required together. |

### Before presenting

From the TrustGraph folder:

```sh
cargo build --locked -p trustgraph-cli
cargo run --locked -p trustgraph-cli -- start --port 9080
```

The live setup uses the ignored `trustgraph.local.json`. See
[live setup](live-workspace.md) if it is not configured. Keep the server running.
Open `.trustgraph-live/state/operator.html` once to establish the operator
session, then open `http://127.0.0.1:9080/#/policy`.

Run this rehearsal check in a second terminal:

```sh
python3 scripts/hackathon-check.py --output /tmp/trustgraph-policy-report.json
```

It checks 14 cases through the live Rust/Hydra HTTP path using the agent token.
It verifies that the active policy, existing work and publication effects stay
unchanged. This check does not run a model. A passing simulation is not an
executed-test receipt.

Make sure the active policy permits only `src/lib.rs`, one changed file, requires
tests and has a 30-minute approval lifetime. These are the initial defaults.
If they differ, inspect and activate the desired settings before rehearsing;
activation invalidates earlier work. Prepare a real Agent run shortly before
presenting so there is a real reviewable fallback if model latency is high.
Keep its patch unapproved. Never erase audit history to reset the presentation.

### Scene 1: Policy · 40 seconds

Say:

> “Customers cannot complete a valid zero-value authorization. I want the agent
> to fix that function, but urgency should not give it permission to change the
> rest of the payment system.”

Show the active scope, file budget, test requirement and approval lifetime.
Point to the three always-enforced guarantees: inherited source permissions,
exact approval binding, and complete captured inputs.

Say:

> “The policy is our release boundary. Those guarantees are built into the Rust
> engine; I cannot switch them off by changing this form.”

Click **Test in Simulator →**. This transfers a candidate without activation.

### Scene 2: Simulator · 75 seconds

Click **Code + docs**. The proposed files become `src/lib.rs, README.md`.
Keep the candidate identical to the active policy and click **Compare policies**.
Both results are blocked: the proposal exceeds the file budget and scope.

Edit the candidate:

- Name: `Reviewed code and documentation`
- Allowed files: `src/lib.rs, README.md`
- Maximum changed files: `2`
- Required tests: checked
- Approval lifetime: `30`

Click **Compare policies**. The active policy is **Blocked** and the candidate
is **Allowed**, for the same proposed change and explicit assumptions.

Say:

> “A release note may be reasonable, but I can test that expanded scope before
> giving it to a live agent. Our current release policy has not changed.”

Open **Impact on existing agent changes** if time allows. This compares saved
patches with candidate business rules; a changed policy requires fresh generation
and review in the current live workflow. It does not retroactively authorize a
previous patch. The full runtime still checks permissions and approval.

Point to **zero publication effects** and the Hydra evidence under the results.
HydraDB stores the exact candidate and its relationship to this simulation.

### Scene 3: Simulator · 35 seconds

Click **Patch changed**, then **Compare policies**. Even the wider candidate is
blocked with `STALE_APPROVAL`.

Say:

> “The file is allowed, but the deliverable changed after approval. Permission
> to work on a file and approval to release its exact contents are separate.”

For a backup variation, choose **Policy changed**, **Missing input** or
**Expired approval**. Each should show a specific failure reason. Do not run all
variations in a short pitch. They are useful for judge questions.

### Scene 4: Agent · 60–90 seconds

Open **Agent**. Its work order is already populated. Click **Run Codex**, or use
the terminal command below. The agent runs under the actual active policy;
testing an expanded candidate did not activate it.

Show the live stages: HydraDB inputs → captured plan and manifest → Codex → Rust
checks → review. The useful code change is `minor_units > 0` becoming
`minor_units >= 0`, with a new zero-value regression test.

Show **Passed · 3 tests** and expand **View actual test output** if asked.
Positive and negative tests must remain. These tests actually executed against
this patch; the Simulator's test result was an assumption.

Say:

> “Now the agent has made a real patch. TrustGraph captured the supplied source
> bytes, and the isolated host runner executed the resulting Rust tests. Passing
> tests still do not give the agent release authority.”

The outer Codex chat or CLI submits a work order. TrustGraph starts a fresh,
controlled Codex generation with captured context and external tools disabled.
The host executes the checks. This is a narrow coding workflow, not unrestricted
repository access or a third-party agent's self-reported lineage.

### Scene 5: Review and gate · 45 seconds

Inspect the exact patch and tests, then click **Approve exact change**. The app
shows progress while storing and reading back the connected authorization
records in HydraDB. **Run publication gate** becomes available.

Click it. Show **Gate passed · action simulated** and the new activity entry.
No branch or pull request is created by this action.

Say:

> “Approval names this exact patch, policy and repository. Before acting, the
> gate reads the connected evidence from HydraDB and Rust checks current
> authorization again. The agent cannot approve itself.”

Close with:

> “We can let an agent move quickly while keeping release decisions specific,
> reviewable and testable.”

## HydraDB's role, explained in one click

Open **How TrustGraph works**, then **What the HydraDB records mean**.

| Record | What it represents in this example |
| --- | --- |
| Source | Exact committed bytes supplied to the broker |
| Plan and manifest | The task and complete list of captured inputs |
| Patch and test receipt | Generated content and the tests executed against it |
| Candidate and simulation | A proposed policy and its hypothetical Rust decisions |
| Approval | The authorized operator's exact patch/policy/repository binding |
| Outcome | The verified gate decision and whether an action was simulated |

The required governance graph is in database `trustgraph`, BYOG collection
`trustgraph-governance`. Rust enforces policy. The local journal persists the
coordinator; HydraDB provides input bytes and connected evidence that are checked
on the decision path. Missing or modified graph evidence blocks that path.

The normal Knowledge source list contains five optional browsing copies. It is
not the governance graph, and the entire repository was not ingested. Knowledge
indexing status can be queued independently of the BYOG workflow.

For this small case a local database could hold the records. HydraDB's graph
becomes particularly useful as shared inputs connect many derived artifacts and
review decisions. Do not claim that this owner-only workbench has imported a
bank's department ACLs or runs the confidential-investigation story live.

## Try it from Codex

Open this repository in the Codex app and paste
[the simulator trial prompt](../examples/codex-simulator-trial.md). It asks Codex
to run the live policy suite, author a candidate JSON file and compare outcomes.
The agent token is read by the CLI; no credential value belongs in a prompt.

For a direct terminal experiment:

```sh
cargo run --locked -p trustgraph-cli -- policy-simulate \
  --case examples/payment-change.json \
  --policy examples/payment-policy-expanded.json \
  --expect allowed \
  --output /tmp/trustgraph-comparison.json
```

Expected: active **blocked**, candidate **allowed**, zero code execution,
no policy activation and zero publication effects. The result includes a
candidate-policy graph ID, simulation graph ID and counterexamples.

Then submit real work:

```sh
cargo run --locked -p trustgraph-cli -- run \
  --goal "Fix valid_amount to accept zero, reject negatives and add a zero regression test. Preserve existing tests. Only change src/lib.rs."
```

The CLI streams actual activity and prints the review URL. Review and approval
happen in the operator UI. Another agent can use the same `POST /api/work` or
`POST /api/simulate` HTTP path with the restricted agent token. The older MCP
bridge points to the separate fixture server, not this live 9080 workspace.

Codex scripting uses `codex exec`; JSONL events and structured output are
documented in the [official non-interactive guide](https://learn.chatgpt.com/docs/non-interactive-mode).
The broker already uses that interface. A standalone Codex script also needs
permission to call the local TrustGraph service; the desktop trial is convenient
for observing and approving those tool calls.

## Supported policy experiments

The supported JSON schema is deliberately small:

```json
{
  "name": "Reviewed code and documentation",
  "allowed_paths": ["src/lib.rs", "README.md"],
  "max_changed_files": 2,
  "require_tests": true,
  "approval_minutes": 30
}
```

You can test different safe relative paths, budgets from 1–10 files, test
requirements and lifetimes from 1–1440 minutes. Proposed changes can vary paths,
assumed test results, manifest completeness and approval state: exact, absent,
old patch, old policy or expired. Set approval age to compare lifetimes fairly
(for example, a 20-minute-old approval passes 30 minutes but fails 15). `payment-policy-custom.json` and
`payment-custom-change.json` illustrate paths outside the live workbench.
Unknown fields, arbitrary English rules, wildcards and executable policy code
are unsupported. The live runner/publisher still supports only `src/lib.rs` and
`README.md`; simulation does not expand that capability.

## Rehearsal and recovery

- Use the prepared real run if model latency exceeds the presentation window.
  State that it was executed earlier; do not present a replay as a new generation.
- If HydraDB is unavailable, say the evidence dependency is unavailable. Do not
  substitute fixture results and call the run live.
- If approval expired, inspect and approve the unchanged exact patch again.
  Renewed review creates a new approval record. Policy changes require a fresh
  generation rather than renewal of the old policy context.
- Keep real PR creation out of the five-minute story. It is a separate optional
  action and Composio is not configured.
- To repeat, submit a new work order. Simulated publication leaves the workbench's
  `main` unchanged, so the original bug remains a reproducible exercise. Existing
  evidence stays in the journal and HydraDB.

## Recorded rehearsal evidence

- [Live policy suite: 14 checks](../evidence/hackathon-policy-checks.json)
- [Active/candidate comparison](../evidence/hackathon-policy-comparison.json)
- [Comparison screen](../evidence/hackathon-policy-comparison.png)
- [Stale-approval screen](../evidence/hackathon-stale-approval.png)
- [Real Codex run, three executed tests and simulated gate](../evidence/hackathon-agent-run.json)

These are records from the configured local workspace, not instructions to
substitute recorded outcomes for a fresh live run.

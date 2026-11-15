# TrustGraph as policy-as-code for what agents produce

Status: draft for approval · 2026-11-15

## Intent

Reposition TrustGraph from "a Rust lineage framework plus several demos" to one
product: **a policy file and decision engine that says where an agent's output
may go, based on what the agent read.**

Why: the owner wants teams to adopt TrustGraph inside their own agent systems,
and expects **agents to write most policies**. The current repo has three
policy formats, a Rust-only in-process API and a README that opens with
mechanism instead of the problem.

The one idea that sets it apart: restrictions follow the data. A summary of
HR-only salary data is still HR-only, whatever tool the agent uses next.
Approval can satisfy a review rule; it can never widen who may read a source.

## Goals

1. One policy file format that a person or an agent can write correctly on the
   first or second try.
2. Tooling that lets an agent check its own policy without a human: validate,
   test, simulate, explain, diff — all with machine-readable output.
3. A language-neutral way to use the engine: a local service over HTTP and MCP.
4. A README and one demo that explain problem → policy → decision in under a
   minute.
5. A smaller core: Composio is an integration, not part of the engine. The
   Composio / 8090 issue flow moves under `examples/`.

## Non-goals

- Proxying tool calls or holding third-party credentials (the "full gateway").
  TrustGraph decides; the host system acts on the decision.
- Published Python / TypeScript packages. v1 ships HTTP, MCP, the Rust crate
  and a short copy-paste Python client in `examples/`.
- Migrating the two reference applications (guided story demo, live Codex
  gateway) to the new policy format. They keep their current formats and move
  out of the README's main path.
- Multi-tenant hosting, user management, a database backend.

## Anti-goals

- An agent must never be able to activate or widen a policy by itself.
- No natural-language rules, no LLM in the decision path. Decisions stay
  deterministic and replayable.
- The README must not promise anything that is not in the repo.

## Approaches considered

- **A. Policy-as-code for agent output (chosen by the owner).** One YAML file,
  a decision service, agent-friendly authoring tools.
- **B. Rust data-lineage library.** Current direction. Honest but narrow;
  most agent stacks are not Rust, and it does not read as "policies for agents".
- **C. Full agent gateway.** Holds every credential and proxies every tool
  call. Strongest boundary, largest build, crowded space. A grows into C later.

## The policy file

`trustgraph.policy.yaml`. Strict: unknown fields are errors.

```yaml
version: 1

groups:                       # people
  hr: [nina]
  engineering: [sam, ravi]
  sales: [leo]
  customers: [acme]

agents: [planner, mailer]     # agents are named principals too

sources:
  salaries:
    readers: [hr]             # who may receive anything derived from it
    agents: [planner]         # which agents may take it as input
  roadmap:
    readers: [hr, engineering]
    agents: [planner]
  price_sheet:
    readers: [sales, customers]
    agents: [mailer]

actions:
  post_to_slack:
    agents: [planner]         # who may attempt it
    audience: [engineering]   # who receives the output
  email_customer:
    agents: [mailer]
    audience: [customers]
    approval: { reviewers: [sales], valid_for: 30m }

tests:                        # run by `policy test`; must pass to activate
  - name: salary data never reaches engineering
    output_from: [salaries, roadmap]
    agent: planner
    action: post_to_slack
    expect: deny
  - name: roadmap summary can be posted
    output_from: [roadmap]
    agent: planner
    action: post_to_slack
    expect: allow
```

Semantics (unchanged from the existing SDK; the reader-set kernels it calls in
`trustgraph-core` are the part covered by the Kani proofs):

- `readers(output) = intersection of readers(every input)`.
- An action is allowed only if its audience ⊆ readers(output), the agent is
  listed for the action, and the agent was allowed every input.
- `approval` adds a requirement. It never widens readers.
- Anything not named in the policy is denied (unknown source, action, agent).
- A caller may pass explicit `recipients` on a check; they must be a subset of
  the action's `audience`. Default is the whole audience.

Decisions made during the design review (my calls, open to correction):

- **Groups expand to people.** All subset checks run on expanded members, so
  someone in both `hr` and `engineering` may receive hr-only output. Reasons
  name both the group and the people left out. An empty group is an error.
- **Reviewers must be allowed to see what they approve.** A reviewer has to be
  a reader of the output and cannot be the agent that asked. `policy check`
  rejects an approval rule whose reviewers can never read any output the
  action could carry.
- **Readers always come from the active policy, by source name.** Narrowing a
  source takes effect at once on everything already derived from it. A source
  removed from the policy makes its outputs deny. An approval is tied to the
  sources and action rule it was given under; changing any of them voids it.

Internally a source's reader set is `readers ∪ agents`, which is how the
existing SDK already treats an executor ("the model receiving an input is
itself a recipient").

## Agent-friendly authoring

The design assumption: an agent drafts the policy, a human approves it.

CLI, every command with `--json`:

| Command | What it does |
| --- | --- |
| `trustgraph policy init` | Writes a commented starter file |
| `trustgraph policy schema` | Prints the JSON Schema |
| `trustgraph policy check FILE` | Validates. Errors carry `path`, `code`, `message`, `suggestion` (e.g. "unknown group `enginering`; did you mean `engineering`?") |
| `trustgraph policy test FILE` | Runs the inline `tests` and reports each as pass/fail with the reason |
| `trustgraph policy explain FILE` | Plain-English summary per source and action, for the human reviewer |
| `trustgraph policy diff OLD NEW` | Lists who gains and who loses access; marks every widening |

Design rules for the format and tools:

- Every error says where, what and how to fix it. No error is a bare string.
- All errors are reported at once, not one per run.
- A policy with zero tests gets a warning; a failing test blocks activation.
- Lint warnings for likely mistakes: a source no agent can use, an action
  whose audience can never be satisfied by any source, an approver who is also
  the only agent.
- The same six operations are exposed as MCP tools (`policy_schema`,
  `policy_check`, `policy_test`, `policy_explain`, `policy_diff`,
  `policy_propose`).
- An agent guide ships in the repo: `docs/agents/writing-policies.md`, plus a
  root `llms.txt` pointing to it and the schema. It covers the format, the
  check → test → explain → propose loop and five worked examples.

**Activation is human-only.** `policy_propose` stores a candidate and returns
its diff and explanation; it does not change the active policy. Activation
happens through the operator session (see below) or by the human restarting
`serve` with a new file. Documented limit: if an agent shares the operator's
shell and can edit the file and restart the service, the control is code
review of the policy file (branch protection / CODEOWNERS), which the guide
recommends.

## Using it in a system: `trustgraph serve`

```sh
trustgraph serve --policy trustgraph.policy.yaml --state ~/.trustgraph
```

A local service, loopback only by default. Four calls, over HTTP JSON and as
MCP tools:

| Call | Input | Result |
| --- | --- | --- |
| `register_source` | source name from the policy, content or its sha256 | artifact id |
| `record_output` | input artifact ids, agent, step name, content or sha256 | artifact id |
| `check` | artifact id, action, agent, optional recipients | `allow` / `needs_approval` / `deny`, a reason naming the blocking sources, a decision id |
| `approve` | decision id | approval (operator only) |

Plus read-only `explain_artifact` (lineage and current readers) and
`policy_*` tools above.

- **Reasons are specific:** "denied: `salaries` is readable by hr only;
  engineering is not included", with the list of blocking sources as data.
- **Content is optional.** Identity is the sha256, so a host can keep
  sensitive bytes out of TrustGraph entirely.
- **Agent identity comes from the token, not from a parameter.** `serve`
  writes one token file per agent named in the policy; the caller's agent name
  is whatever its token maps to, so `mailer` cannot claim to be `planner`.
- **The operator is one named person.** `serve --operator leo` (or config)
  says who the human is. Approval and activation use the single-use launch
  file + HttpOnly session already built in `trustgraph-live/src/auth.rs` (move
  it to a shared place), and the operator must be a listed reviewer.
  Multiple signed-in reviewers are out of scope for v1.
- **A small operator page ships with `serve`:** pending decisions awaiting
  approval (what, where, why, lineage) and proposed policies with their diff
  and explanation, each with Approve / Activate. No other UI.
- **After approval the host calls `check` again** and gets `allow`; it does
  not handle approval ids.
- **State:** an append-only journal in the state directory, replayed on start.
  The state directory must be outside agent workspaces (same check as `start`).
- **The host must call `check` immediately before acting** and act only on
  `allow`. TrustGraph does not perform the action. This limit is stated in the
  README.

## One runtime, driven by the simulator

Owner requirement: the simulator must run the runtime itself, not a model of it.

- **The runtime core is a deterministic state machine.** Input: the active
  policy and a stream of events (register source, record output, check,
  approve, revoke, propose / activate policy, clock tick). Output: decisions
  and journal entries. No clock, network or disk inside it; all three are
  passed in.
- **`serve` = core + real I/O** (wall clock, HTTP / MCP, journal file).
- **The simulator = the same core + fake I/O**: a seeded clock, a seeded
  interleaving of concurrent agents and injected faults (crash between journal
  write and reply, torn journal tail, restart and replay, policy change between
  a check and the host's act, approval expiry, revocation races). A failing
  seed replays exactly.
- **Invariants checked on every simulated step:**
  1. No `allow` ever names a recipient outside the readers of every source in
     the output's lineage under the policy active at that moment.
  2. No approval survives a change to its sources or its action rule.
  3. Crash + replay reaches the same state and the same next decisions.
  4. Narrowing a source applies to everything already derived from it.
  5. Nothing an agent token can call changes the active policy.
- **`trustgraph policy simulate FILE [--seed N --count K --json]`** runs a
  user's own policy through seeded scenarios generated from it (including
  proposed-policy activations mid-run) and reports any violation with its seed.
  Also exposed as the MCP tool `policy_simulate`, so an agent can run it on a
  policy it drafted.
- The demo page gets a Simulator panel that calls the same code through the
  hosted endpoint.
- Stated limit: the simulator covers TrustGraph's decisions and state. It
  cannot cover what a host does after `allow`.
- The existing `trustgraph-sim` for the older coordinator stays with the
  reference gateway. Reuse its seeded-schedule, trace and replay machinery
  where practical (left to the builder).

## Repository changes

Move to `examples/integrations/composio-github-issues/` (owner's decision:
Composio is an integration and belongs under examples):

- `crates/trustgraph-cli/src/actions.rs` and `actions/` become that example's
  own small binary crate (a workspace member); the `Actions` command leaves the
  main CLI.
- With it go `web/actions.*`, `scripts/*composio*`, `scripts/8090_demo.py`,
  `scripts/trustgraph_actions.py` and their tests,
  `scripts/test_walkthrough.cjs`, `docs/8090-demo.md`,
  `docs/8090-issue-actions.md`, `docs/one-hour-issue-demo.md`, `examples/8090/`
  and `trustgraph-actions.example.json`, with paths and links fixed and one
  README for the example.
- It moves as it is and keeps passing its tests. Rewiring it to call
  `trustgraph serve` for its decisions is a follow-up, not part of this work.
- The main README links to it from an "Integrations" line: the pattern is
  "your tool layer (Composio, MCP servers, your own code) calls `check` before
  it acts".

Keep, but move out of the README's main path into a "Reference applications"
section: the guided story demo, the live Codex/GitHub gateway, HydraDB
evidence, the simulator and Kani proofs (these last two stay prominent under
"How we know it works").

Add:

- A policy module (suggested: new crate `trustgraph-policy`) that parses,
  validates, tests, explains and diffs, and compiles a policy into the
  existing `trustgraph-framework` graph rules.
- `policy` and `serve` commands in `trustgraph-cli`.
- `examples/policies/` with five worked policies and
  `examples/python_client.py`.

## Demo

One page replaces the current landing page; it calls a stateless hosted
endpoint backed by the same Rust policy module.

```text
┌ Policy (editable YAML) ──┐ ┌ Agent run ───────────────┐ ┌ Decision ───────────────┐
│ sources:                 │ │ Agent:  planner          │ │ ✗ DENIED                │
│   salaries: readers [hr] │ │ Used:   ☑ salaries       │ │ salaries is hr-only;    │
│ actions:                 │ │         ☑ roadmap        │ │ engineering not included│
│   post_to_slack: ...     │ │ Action: post_to_slack    │ │                         │
│ [Check] [Run tests]      │ │ [Check]                  │ │ salaries ─┐             │
│ ✓ 2 tests pass           │ │                          │ │ roadmap ──┴─ output     │
└──────────────────────────┘ └──────────────────────────┘ └─────────────────────────┘
```

Editing the policy re-runs its tests and shows the plain-English diff against
the starting policy. Validation errors appear inline with their suggestions.
The existing story and simulator pages stay reachable by link.

## README outline

1. One-line statement and the problem in three sentences.
2. A 15-line policy and the decisions it produces.
3. Quickstart: `policy init` → `policy test` → `serve` → one `check` call.
4. "Let your agent write the policy" — the loop and the guide link.
5. How it works (the two rules, one diagram).
6. How we know it works (tests, simulator, Kani) and what it does not do.
7. Reference applications; repository layout.

## Build order

1. Move the Composio / 8090 flow under `examples/integrations/`.
2. Policy module + CLI commands + worked examples + agent guide.
3. Deterministic runtime core + simulator + `policy simulate`.
4. `serve` (HTTP, then MCP) on that core + operator page + Python client
   example.
5. Hosted endpoint + demo page (with Simulator panel).
6. README and docs rewrite, written last so it only describes what exists.

## Left to the builder

- Crate boundaries and whether `record_output` needs a simpler recording path
  in `trustgraph-framework` than the current typed-port ticket API.
- Journal format for `serve`.
- Exact lint list beyond the three named above.
- Visual styling of the demo, reusing the existing page's look.

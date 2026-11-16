# TrustGraph

**Policy-as-code for what AI agents produce.**
Decide where your agent's output can go, based on what it read.

[Try the playground](https://trustgraph-demo.vercel.app) ·
[Write a policy](docs/agents/writing-policies.md) ·
[How we know it works](docs/verification.md)

## The problem

Permission systems decide what an agent may **read**. They lose track the
moment the agent **writes** something new.

An agent reads the HR salary file and the product roadmap. It writes a "Q3
headcount plan" and posts it to the engineering channel. The plan is new text.
It has no owner and no access list, so nothing stops it. The salary data just
left HR.

The restriction should have followed the data.

## What TrustGraph does

You write one small policy file. It says who may read anything derived from
each source, and where each action sends output:

```yaml
version: 1

groups:
  hr: [nina]
  engineering: [sam, ravi]
  sales: [leo]
  customers: [acme]

agents: [planner, mailer]

sources:
  salaries:    { readers: [hr],                 agents: [planner] }
  roadmap:     { readers: [hr, engineering],    agents: [planner] }
  price_sheet: { readers: [sales, customers],   agents: [mailer] }

actions:
  post_to_slack:
    agents: [planner]
    audience: [engineering]
  email_customer:
    agents: [mailer]
    audience: [customers]
    approval: { reviewers: [sales], valid_for: 30m }
```

For every output, TrustGraph answers **allow**, **needs approval** or **deny**,
and says why:

| The agent read | It wants to | Answer |
| --- | --- | --- |
| `salaries` + `roadmap` | `post_to_slack` | **Deny.** `salaries` is readable by hr only; engineering (ravi, sam) is not included |
| `roadmap` | `post_to_slack` | **Allow** |
| `price_sheet` | `email_customer` | **Needs approval** from sales, valid for 30 minutes |

The rule is small:

```text
readers(output) = the people who may read every input
an action is allowed only if everyone who receives it is one of those readers
```

Approval adds a requirement. It never widens who may read a source. Anything
the policy does not name is denied.

Decisions are deterministic. There is no model in the decision path.

## Quickstart

Requires Rust 1.90 or newer.

```sh
git clone https://github.com/skishore23/trustgraph && cd trustgraph
cargo install --locked --path crates/trustgraph-cli

trustgraph policy init                         # writes trustgraph.policy.yaml
trustgraph policy check trustgraph.policy.yaml
trustgraph policy test trustgraph.policy.yaml
```

```text
ok: 0 errors, 0 warnings
PASS salary data never reaches engineering
PASS roadmap summary can be posted
2 passed, 0 failed
```

A policy carries its own tests. Each one states what the agent read, what it
tries to do and the answer you expect.

Read a policy back in plain English, and see exactly who gains access when it
changes:

```sh
trustgraph policy explain trustgraph.policy.yaml
trustgraph policy diff old.policy.yaml new.policy.yaml
```

```text
salaries: output can reach hr (nina). Usable by agent planner.
email_customer: mailer may send to customers (acme) after approval by sales (leo), valid 30m.

WIDENS  source salaries: readers gained ravi, sam
```

Run your own policy through the simulator:

```sh
trustgraph policy simulate trustgraph.policy.yaml
```

```text
64 runs, 12,936 events: 994 allow, 5,082 deny, 392 needs_approval — 0 violations
```

Every command takes `--json`. There are five worked policies in
[examples/policies](examples/policies).

## Let your agent write the policy

TrustGraph assumes an agent drafts the policy and a person approves it.

- **Errors an agent can act on.** Every problem is reported at once, with its
  path, a code and a suggestion:

  ```json
  {
    "code": "unknown_name",
    "message": "`enginering` is not a group or person in this policy",
    "path": "sources.salaries.readers[1]",
    "severity": "error",
    "suggestion": "did you mean `engineering`?"
  }
  ```

- **A loop it can run alone:** `init` → `check` → `test` → `simulate` →
  `explain` → `diff`.
- **A review a person can do in a minute:** the plain-English explanation and
  a diff in which every widening of access is marked.

Point your agent at [docs/agents/writing-policies.md](docs/agents/writing-policies.md)
or [llms.txt](llms.txt). Agents propose; people approve. Protect the policy
file with code review.

## Use it in your system

Today TrustGraph ships as Rust crates and the CLI.

```rust
use trustgraph_policy::{decide, parse, Query, Verdict};

let policy = parse(&policy_yaml).policy.expect("valid policy");
let outcome = decide(&policy, &Query {
    output_from: ["salaries", "roadmap"].map(String::from).into(),
    agent: "planner".into(),
    action: "post_to_slack".into(),
    recipients: None, // the action's whole audience
});
assert_eq!(outcome.verdict, Verdict::Deny);
println!("{}", outcome.reason);
```

```sh
cargo run -p trustgraph-policy --example decide
```

| Crate | Use it for |
| --- | --- |
| [`trustgraph-policy`](crates/trustgraph-policy) | Parse, validate, test, explain and diff policies. `decide` is a pure function. |
| [`trustgraph-engine`](crates/trustgraph-engine) | A journaled runtime that records sources and outputs, tracks approvals and survives restarts. |

**Coming next:** `trustgraph serve`, a local service over HTTP and MCP, so any
agent stack can register what it read and ask before it acts. It is not built
yet.

## How we know it works

- **One decision order.** The allow / needs-approval / deny ordering lives in
  one small kernel that both the policy function and the runtime call.
- **Bounded proofs of the core rules.** Ten [Kani](https://model-checking.github.io/kani/)
  harnesses cover that kernel, the approval time window and the reader-set
  rules. All ten pass in CI.
- **A simulator that runs the real runtime.** Seeded runs drive the same code
  through policy changes, crashes, torn journal writes and approval expiry,
  and check five safety rules on every step against a check computed from the
  policy text alone. Three deliberately broken builds of the runtime must be
  caught, and are.

The proofs are bounded and cover the core rules, not the whole system. The
simulator shows the absence of violations in the runs it made, not in all
possible runs. [Verification scope and reproduction](docs/verification.md)
states exactly what each covers.

## What it does not do

- **It only knows what you tell it.** If your system does not report a source
  the agent read, TrustGraph cannot account for it.
- **It decides; it does not act.** Your system must ask right before it acts,
  and act only on *allow*.
- **The rule is strict.** One sensitive input restricts the whole output to
  that input's readers. There is no "this summary is safe to share" exception
  yet.
- **Groups are lists in the file.** They are not synced from an identity
  provider.

## Where it fits

Permission databases such as SpiceDB or OpenFGA decide what your agent may
read. TrustGraph decides where what it wrote may go. Use both.

## Repository

| Path | What it is |
| --- | --- |
| `crates/trustgraph-policy` | The policy file: parsing, decisions, tests, explain, diff |
| `crates/trustgraph-engine` | Deterministic runtime core, journal and replay |
| `crates/trustgraph-core` | Proof-friendly kernels and their Kani harnesses |
| `crates/trustgraph-sim` | The seeded simulator and its safety checks |
| `crates/trustgraph-cli` | The `trustgraph` command |
| `examples/policies` | Five worked policies |
| `docs/agents` | The policy-writing guide for agents |

### Reference applications

Earlier work that TrustGraph grew out of. They use their own policy formats.

- **Typed dataflow SDK** — `crates/trustgraph-framework`, the lineage graph the
  runtime is built on. [Guide](docs/framework.md) ·
  [interactive lab](https://trustgraph-demo.vercel.app/lab.html)
- **Guided story demo** — a fictional company walkthrough.
  [Demo](https://trustgraph-demo.vercel.app/story.html) ·
  [walkthrough](docs/demo.md)
- **Live coding gateway** — a fresh Codex run, isolated Rust checks, human
  approval and a GitHub draft pull request. [Architecture](docs/architecture.md) ·
  [setup](docs/development.md)
- **Composio integration** — an agent proposes a GitHub issue; a person
  approves it. [Example](examples/integrations/composio-github-issues)

## Development

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

See [development checks](docs/development.md#local-checks) and
[public demo deployment](docs/vercel.md).

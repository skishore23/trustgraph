# TrustGraph

**Policy-as-code for what AI agents produce.**
Decide where your agent's output can go, based on what it read.

[Try the playground](https://trustgraph-demo.vercel.app) ·
[Write a policy](docs/agents/writing-policies.md) ·
[How we know it works](docs/verification.md)

## The problem

Permission systems decide what an agent may **read**. They lose track the
moment the agent **writes** something new.

In May 2025, Invariant Labs
[showed](https://invariantlabs.ai/blog/mcp-github-vulnerability) a coding
agent reading a user's private repositories and publishing what it found in a
pull request on a public one, after it read a crafted issue. Every step was
something the agent was allowed to do. Reading private code was fine. Opening
a public pull request was fine. Nothing checked that the second came from the
first.

Taking away access does not fix this: the agent needs the private code to do
its job. What is missing is a rule about where things derived from it may go.
The same gap opens each time one agent hands its work to another: the
restriction should follow the data through every step.

## What TrustGraph does

You write one small policy file. It says who may read anything derived from
each source, which agents may use it, and where each action sends output:

```yaml
version: 1

groups:
  hr: [nina]
  engineering: [sam, ravi]
  leadership: [maya]

agents: [researcher, planner]

sources:
  salaries:   { readers: [hr],                          agents: [researcher, planner] }
  roadmap:    { readers: [hr, engineering, leadership], agents: [planner] }
  benchmarks: { readers: [hr, engineering, leadership], agents: [researcher, planner] }

actions:
  post_to_engineering: { agents: [planner], audience: [engineering] }
  brief_hr:            { agents: [planner], audience: [hr] }
  brief_leadership:
    agents: [planner]
    audience: [leadership]
    approval: { reviewers: [hr], valid_for: 1h }
```

TrustGraph records what each output was built from, through every step, and
answers **allow**, **needs approval** or **deny** with the reason:

| What the output carries | It wants to | Answer |
| --- | --- | --- |
| planning notes (from `salaries`) + `roadmap` | `post_to_engineering` | **Deny.** `salaries` is readable by hr only; engineering (ravi, sam) is not included |
| the same summary | `brief_hr` | **Allow** |
| `roadmap` only | `post_to_engineering` | **Allow** |
| `roadmap` + `benchmarks` | `brief_leadership` | **Needs approval.** action `brief_leadership` requires a reviewer to approve it |

The full file, with its tests, is
[examples/policies/handoff.policy.yaml](examples/policies/handoff.policy.yaml).

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
cargo install --locked --git https://github.com/skishore23/trustgraph trustgraph-cli

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

Read a policy back in plain English, see exactly who gains access when it
changes, and run it through the simulator:

```sh
trustgraph policy explain trustgraph.policy.yaml
trustgraph policy diff old.policy.yaml new.policy.yaml
trustgraph policy simulate trustgraph.policy.yaml
```

```text
salaries: output can reach hr (nina). Usable by agent planner.
email_customer: mailer may send to customers (acme) after approval by sales (leo), valid 30m.

WIDENS  source salaries: readers gained ravi, sam

64 runs, 12,936 events: 994 allow, 5,082 deny, 392 needs_approval — 0 violations
```

Every command takes `--json`. There are six worked policies in
[examples/policies](examples/policies).

## Let your agent write the policy

TrustGraph assumes an agent drafts the policy and a person approves it. Every
problem is reported at once with a path, a stable code and a suggestion, so an
agent can run the loop `init` → `check` → `test` → `simulate` → `explain` →
`diff` on its own. A person then reviews the plain-English explanation and a
diff in which every widening of access is marked.

Point your agent at [docs/agents/writing-policies.md](docs/agents/writing-policies.md)
or [llms.txt](llms.txt). Protect the policy file with code review.

## Use it in your system

### `trustgraph gateway`: the option agents cannot skip

Put the gateway between your agents and the MCP servers they use. It holds
the servers' credentials, records what each agent reads through them, and
checks every action against it before forwarding the call. A `gateway.yaml`
says which tool reads which source and which tool is which action:

```sh
trustgraph gateway --policy trustgraph.policy.yaml --operator leo --config gateway.yaml
```

```text
planner slack__post_message #engineering: error: TrustGraph gateway: `post_to_engineering` was not forwarded: denied: `salaries` is readable by hr only; engineering (ravi, sam) is not included (denied)
```

Config, sessions, hand-offs and the threat model:
[docs/gateway.md](docs/gateway.md). A runnable example:
[examples/gateway](examples/gateway).

### `trustgraph serve`

A local service over HTTP and MCP, so any agent stack can register what it
read, record what it wrote and ask before it acts. Each agent gets its own
token; a person, the operator, approves requests on a local page.

```sh
trustgraph serve --policy trustgraph.policy.yaml --operator leo
```

Ask right before acting:

```sh
curl -s localhost:8787/v1/check \
  -H "Authorization: Bearer $(cat ~/.trustgraph/serve/tokens/mailer.token)" \
  -H "Content-Type: application/json" \
  -d '{"artifact": "<artifact id>", "action": "email_customer"}'
```

The API, the MCP endpoint, tokens, the operator page, limits and the threat
model are in [docs/serve.md](docs/serve.md).

### From Python

Mark what reads a source and what acts; inside a session, what was read
follows what is written ([SDK reference](docs/sdk-python.md)):

```python
import trustgraph

client = trustgraph.Client(token_file="~/.trustgraph/serve/tokens/planner.token")

@trustgraph.source("salaries")
def read_salaries():
    return "nina: 120k, sam: 95k"

@trustgraph.action("post_to_slack", content=lambda args, kwargs: args[0])
def post_to_slack(text):
    print("posted:", text)               # runs only on allow

with client.session():
    read_salaries()
    try:
        post_to_slack("Q3 headcount plan")
    except trustgraph.Denied as e:
        print(e.decision.verdict, e.decision.reason)
```

```text
deny denied: `salaries` is readable by hr only; engineering (ravi, sam) is not included
```

Integrations, each with a hand-off example: [LangChain / LangGraph, OpenAI
Agents SDK and Claude Agent SDK](docs/integrations.md). A wrapper checks only
the tools it wraps.

### Rust crates

[`trustgraph-policy`](crates/trustgraph-policy) parses, tests, explains and
diffs policies, and `decide` is a pure function (`cargo run -p
trustgraph-policy --example decide`). [`trustgraph-engine`](crates/trustgraph-engine)
is the journaled runtime `trustgraph serve` runs.

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
- **It decides; it does not act.** Except through the gateway, your system
  must ask right before it acts, and act only on *allow*.
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
| `crates/trustgraph-policy` | The policy file: parsing, decisions, tests, explain, diff, schema, starter |
| `crates/trustgraph-engine` | Deterministic runtime core, journal and replay |
| `crates/trustgraph-core` | Proof-friendly kernels and their Kani harnesses |
| `crates/trustgraph-framework` | The lineage graph the engine is built on |
| `crates/trustgraph-sim` | The seeded policy simulator and its safety checks |
| `crates/trustgraph-cli` | The `trustgraph` command: `policy`, `serve` and `gateway` |
| `sdk/python` | The Python SDK and its framework integrations |
| `examples/policies` | Six worked policies |
| `examples/gateway` | The hand-off story through the gateway |
| `examples/integrations` | A hand-off example per integration |
| `docs/agents` | The policy-writing guide for agents |
| `web`, `api`, `hosted` | The public site and its playground endpoint |

## Development

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

See [development](docs/development.md) and [the public site](docs/vercel.md).

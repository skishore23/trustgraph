# TrustGraph

A Rust compiler for safe agent plans. Before an agent acts, TrustGraph checks how
its proposed reads, generation, approval and publication compose. It identifies a
concrete recipient/source conflict, offers permitted alternatives, then enforces
exact output and destination bindings when the selected plan runs.

The default workflow is now **one coding agent, two bank departments**. Engineering
requirements are shareable; a Fraud & Risk investigation is not. TrustGraph blocks
a derived patch from entering Engineering's repository, offers a fresh-context
repair and requires approval of the actual artifact and destination.

The [small explicit policy layer](docs/policies.md) lets you author versioned YAML,
run five deterministic cases and activate a reviewed policy atomically. A coding
agent can propose and test policies through MCP, but cannot activate or approve.
See the [bank walkthrough](docs/bank-demo.md) and [policy examples](examples/bank-policy.yaml).

The app uses synthetic documents, fixture patch generation and local simulated
delivery. **Real isolated coding-agent input capture and the gated Composio branch/PR
publisher are not implemented yet.** Hydra supports namespace configuration,
projection and fully validated historical bundle reconstruction; live qualification
awaits the service details. [Composio contract](docs/composio-plan.md).

The earlier [consulting business case](docs/business-case.md) remains available as
`--scenario consulting`; existing data is preserved. This is one workspace per
running app, without tenancy or billing.

## Run

Requires Rust 1.90 or newer. Built and tested with Rust 1.97.1. Dependencies are
locked in `Cargo.lock`; no Node server, model API key or database is needed for
the local workflow.

```sh
cargo run --locked -p trustgraph-cli -- policy-check examples/bank-policy.yaml
cargo run --locked -p trustgraph-cli -- serve
```

Open [the permission simulator](http://127.0.0.1:8080/#/simulator) as Arun for the
Risk view, or Maya for Engineering. Select evidence and choose Engineering’s
repository or the Risk review room. The real
Rust compiler powers the verdict and document/recipient permission matrix.

Keep the audience and omit unshareable evidence, or keep the evidence and restrict
the audience. A passing selection can **Prepare memo for review**. The broker
records the actual inputs and content, then requires a human to **Approve & release**
the exact version and destination. Reviews, Deliveries and Sources support this
workflow. Typed Agent plans and bounded Verification remain under Advanced.

The **Verification** page explores the bounded model and can show a deliberate
approval-reuse fault with a shortest counterexample. This changes the model only,
never the running app's policy.

The browser uses fixed demo identities (`demo-maya`, `demo-arun`, `demo-sam`,
`demo-agent`). The server binds only to loopback, checks origin/host and authorizes
each API request. These are development identities, not production authentication.
Agent identity cannot approve. Sam cannot retrieve private source or draft data.

The default bank workspace survives restarts in `.trustgraph-bank/`. The consulting
workspace remains in `.trustgraph-consulting/`; use `serve --scenario consulting`
to open it. Existing
Northstar data in `.trustgraph/` is preserved; do not reuse that directory for the
consulting scenario. To start a new seeded workspace without
deleting evidence, stop the current server and run:

```sh
demo_directory=$(mktemp -d /tmp/trustgraph-demo.XXXXXX)
cargo run --locked -p trustgraph-cli -- serve --data "$demo_directory"
```

## Use it from an agent

TrustGraph now serves real MCP through the official Rust SDK. Keep the app running,
then add [the local configuration](examples/mcp.local.json) to your client's MCP
settings, or download the configuration from **Connect your agent** in the app.
The stdio command connects to the same running workspace; it does not start a
second journal owner. Agents can inspect, simulate, propose, analyze, repair,
prepare and read status. Approval stays in the reviewer app.

[Try MCP](docs/mcp.md) includes a trial prompt and transport contract.
[8090 Software Factory integration](docs/8090-integration.md) explains the working
companion-agent approach and what remains before a direct hosted-agent deployment.
Real 8090 documents and ACLs are not imported by this synthetic integration.

## Stages and current status

| Stage | Deliverable | Status |
| --- | --- | --- |
| Product demo | Bank Engineering/Risk patch, source/audience simulator and guarded review | Implemented with synthetic data |
| Policy layer | Explicit YAML, immutable versions, five-case suite, atomic human activation and MCP proposal/testing | Implemented |
| 1. Typed compiler | Read/Generate/Approve/Publish contracts, ordered references, explicit input manifests and runtime approval obligation | Implemented |
| 2. Counterexample and repairs | Concrete forbidden recipient/source/path; narrower audience or fresh permitted context; no ACL changes | Implemented and tested |
| 3. Controlled plan execution | Journaled proposals and run reservations, exact source/output bindings, resumable generation, human review and guarded dispatch | Implemented with local fixture generation/publishing |
| 4. Finite verification | Breadth-first bounded exploration, independent invariants and a model-only approval-reuse mutant | Implemented; scoped bounds are explicit |
| 5. TrustGraph MCP | Agent-facing propose/analyze/repair/prepare/status transport; no approval or raw publisher credential access | Implemented with authenticated loopback MCP and stdio bridge; live third-party registration pending |
| 6. Hydra persistence | Store plans/run relationships and load validated graph evidence from a real server | Existing projection/protocol tests; live plan/evidence backend pending |
| 7. Composio action | One real dedicated destination with server-owned credentials, idempotency and outcome reconciliation | Pending qualification; publishing is simulated |
| 8. Narrow formal tools | Optional Kani authorization-function checks, then broader least-authority synthesis research | Planned |

The current [bank/policy design](docs/superpowers/specs/2026-11-14-bank-policy-design.md)
builds on the [compiler design](docs/superpowers/specs/2026-11-14-plan-compiler.md)
and supersedes the earlier demo framing. The existing pure verifier,
immutable content, permission-filtered evidence, journal and controlled publisher
remain the underlying execution machinery. [Integration gates](docs/next-stages.md)
record what is not yet live-qualified.

## Test and simulate

```sh
cargo run --locked -p trustgraph-cli -- compile-plan examples/launch-plan.json
cargo run --locked -p trustgraph-cli -- check-model --depth 6
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo run --locked -p trustgraph-cli -- simulate --seed 0 --count 64 --output /tmp/trustgraph-campaign.json
cargo run --locked -p trustgraph-cli -- replay /tmp/trustgraph-campaign.json
```

`check-model` exhausts the declared finite abstraction within its depth bound.
The depth-6 normal run explores 103 nonterminal states and 624 transitions. Use
`--broken-rule` to demonstrate a counterexample; this command intentionally exits
with status 1 when a violation is found. It does not mutate production policy.

`simulate` runs all eight seeded scenarios by default. Select one with, for example,
`--scenario acl-race`. Scenarios cover the complete demo, ACL and revocation
ordering, ambiguous provider acknowledgment and restart, journal uncertainty,
approval expiry, audience changes and policy changes. See [simulator details](docs/simulator.md)
and the [verification record](evidence/README.md).

## Workspace

| Crate | Responsibility |
| --- | --- |
| `trustgraph-core` | Typed plan compiler and repairs; schema/lineage checks, finite reader policy, approval binding and replay |
| `trustgraph-runtime` | Trusted generation, append-only storage, effect coordinator, simulated publisher and Hydra projection |
| `trustgraph-sim` | Exhaustive finite checker plus reproducible virtual schedules and declared faults over the runtime |
| `trustgraph-cli` | CLI, loopback Axum API and embedded HTML/CSS/JavaScript review desk |

Exact text hashes use SHA-256 over UTF-8 bytes. Structured hashes use compact
sorted-key JSON, ordered arrays, explicit null fields, integer timestamps and a
domain/schema envelope. Artifacts never change in place. A source's current ACL
pointer can advance while previous ACL revisions remain historical evidence.

The publication lock covers current verification, persisted reservation,
persisted dispatch, provider call and observed outcome. Repeated keys return the
same logical operation; using the key with different context conflicts. An
uncertain operation is reconciled by lookup and is never resent automatically.
An operation left reserved at recovery is confirmed unsent; one left dispatching
becomes unknown. A corrupt or torn journal blocks startup.

## Hydra

No Hydra instance was available during this build. The local journal is the
authority and Hydra is an optional, connected evidence projection. It is not yet
the runtime's evidence-loading backend. [Hydra setup and qualification](docs/hydra.md)
documents the real roundtrip command and its remaining checks.

## Scope

This is a synthetic development MVP, not a production security certification.
The fixed generation template demonstrates provenance plumbing, not goal understanding or LLM behavior. Plan goals are descriptive metadata; the compiler does not infer actions from natural language. The virtual
scheduler controls commands and modeled faults, not arbitrary threads or network
packets. Export digests detect inconsistent edits; without signatures and a trusted
collector they do not prevent an adversary from rewriting an entire history.
Historical replay explains recorded authorization at recorded time. It grants no
current publishing permission and cannot recall content already delivered.

No Hydra engine source is embedded or relicensed here. Application licensing has
not been selected. Choose it separately before distribution.

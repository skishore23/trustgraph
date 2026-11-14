# TrustGraph

One coding agent can help two departments without making everything it learns
available to both. TrustGraph is a Rust policy and publication gateway that
captures generation inputs, follows their permissions, binds human approval to
an exact change and gates a real GitHub draft pull request.

## The working case

A bank is modernizing payment reconciliation. Engineering owns implementation
requirements. Fraud & Risk owns a confidential investigation. The same coding
agent may help both, but Engineering cannot read the investigation.

1. Generate a patch using both sources. Actual tests may pass, but publication
   to Engineering is blocked by the Risk-only dependency.
2. Start a fresh generation from Engineering requirements and an approved Risk
   specification. The restricted patch is not reused.
3. Approve the exact new patch. Revise its actual bytes and rerun tests: the old
   approval no longer matches.
4. Approve again. The gate reads connected evidence from HydraDB, rechecks Rust
   authorization and creates a real branch and draft PR in the private workbench.

The bank documents are controlled test material; Codex, HydraDB, executed Rust
checks and the GitHub publisher are live. The configured department audiences
are an explicit model, not an import of enterprise IAM. The local operator is a
Risk-authorized reviewer/source owner. Composio is not configured; the qualified
publisher currently uses authenticated GitHub API calls through `gh`.

## Run

Requirements: Rust, signed-in `hydradb`, `codex` and `gh` CLIs, and macOS
`sandbox-exec` for the isolated Rust checks. Configure `trustgraph.local.json`
using [the setup guide](docs/live-workspace.md), then:

```sh
cargo run --locked -p trustgraph-cli -- start
```

Open the private `operator.html` launch file printed at startup once to sign in.
The app is at [localhost:8080](http://127.0.0.1:8080/), with **Workspace**,
**Policy** and **Simulator**. Appearance is fixed ASCII; no theme or demo route
is needed. Old theme/demo URLs normalize to the app root.

Try the same controlled agent intake from Codex or a terminal:

```sh
cargo run --locked -p trustgraph-cli -- run --include-risk \
  --goal "$(cat examples/bankcase/work-order.txt)"
```

A blocked exit is expected with Risk context. Use **Generate from approved
inputs** in the app for a new isolated run. The agent credential cannot activate
policies, approve, publish, or introduce an unadmitted work order as public input.
An operator can admit a new task by submitting it in Workspace.

## Policy experiments

Policies are the rules and parameters. The Simulator tests them across cases;
it does not publish code. Edit source readers, approval lifetime, required tests
or file scope. Compare a candidate with the active policy, or **Run all scenarios**.

```sh
python3 scripts/bank-policy-rehearsal.py
cargo run --locked -p trustgraph-cli -- policy-simulate \
  --case examples/bankcase/risk-to-engineering.json \
  --policy examples/bankcase/policy-source-grant.json --expect allowed
```

The 16-case rehearsal covers department sharing, changed patch/audience/policy,
missing and expired approval, unknown lineage, failed tests, source grants,
shorter lifetimes and custom file scopes. It records evidence in HydraDB without
changing active policy, saved jobs or GitHub. Supported structured policy fields
are explicit; arbitrary English rules are rejected. Source inheritance, complete
lineage and exact approval binding are system guarantees.

## Architecture

```text
CODING AGENT -- HTTP / CLI --> PLAN + INPUT MANIFEST
                                      |
                         RUST POLICY / LINEAGE / APPROVAL
                              |                 |
                          SIMULATOR           RUNTIME
                        counterexample          |
                                         PUBLICATION GATE
                                                |
                                         BRANCH + DRAFT PR

HYDRADB: sources -> permissions -> captured generation -> patch -> approval -> outcome
```

HydraDB is a required evidence store and readback dependency. Rust evaluates
permissions and exact bindings; a durable runtime journal tracks publication
attempts. GitHub credentials remain in the gateway, outside the generated code's
execution sandbox. This controls the brokered workflow; it cannot constrain an
external agent that independently holds GitHub credentials or unseen context.

[Five-minute walkthrough](docs/hackathon-walkthrough.md) ·
[Live setup and limits](docs/live-workspace.md) ·
[Film and production sources](media/trustgraph-film/README.md)

Earlier offline fixtures remain available through `serve`, the bounded state
checker through `check-model`, and the fixture MCP bridge through `mcp`. These
are separate development paths. The live coding-agent integration currently
uses HTTP/CLI; a live MCP transport and Composio adapter remain future work.

## Verification

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all --check
```

The local suite passes 85 tests, with one standalone-Hydra contract test requiring
an explicitly configured external fixture. The separate live rehearsal qualifies
the hosted Hydra path. [Current qualification](docs/live-qualification.md) records
the real agent, stale-approval check and actual draft PR.

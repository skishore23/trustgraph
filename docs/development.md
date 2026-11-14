# Build and live setup

## Local checks

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo run --locked -p trustgraph-cli -- check-model --depth 6
cargo run --locked -p trustgraph-cli -- simulate --seed 0 --count 64
```

The suite exercises access inheritance, exact approval, expiry/revocation,
lineage, policy changes, recovery, deterministic campaigns, the guided story
and the live MCP adapter over a real stdio client with a mock HTTP backend.
An opt-in standalone-Hydra contract test is ignored unless explicitly configured.
No external publishing is performed by the normal test suite.

Useful advanced commands: `compile-plan examples/launch-plan.json` analyzes a
typed fixture plan;
`simulate --output /tmp/campaign.json` saves virtual schedules;
`replay /tmp/campaign.json` verifies them. `check-model --broken-rule` deliberately
finds a stale-approval counterexample within the finite model. These are bounded
checks, not a proof about every possible deployment.

## Start the live workspace

Requirements: Rust; authenticated `codex`, `hydradb` and `gh` CLIs; macOS
`sandbox-exec` for isolated Rust checks; and a dedicated private GitHub repository
owned by the signed-in personal account. The publisher verifies repository
privacy and collaborators. The current runner is intentionally a small Rust
workbench, not an executor for arbitrary projects or build scripts.

Save ignored `trustgraph.local.json`, replacing all paths and the target name:

```json
{
  "directory": "/absolute/TrustGraph/.trustgraph-live/bank",
  "context_repository": "/absolute/TrustGraph",
  "context_files": [
    "examples/bankcase/engineering-requirements.md",
    "examples/bankcase/risk-investigation.md",
    "examples/bankcase/approved-specification.md"
  ],
  "target_repository": "/absolute/private-workbench",
  "github_repository": "your-account/private-workbench",
  "departments": true
}
```

The target must be a clean Git checkout of that private repository, with the small
Rust fixture in `src/lib.rs`. Source imports read **committed Git bytes** from
`context_repository`. Commit controlled inputs before importing them. Use a new
state directory for a different repository or audience; only one server may own
it. Department-mode evidence uses the `bank-departments` namespace.

```sh
cargo build --locked -p trustgraph-cli
./target/debug/trustgraph start
```

Open the private `operator.html` file printed at startup once. It establishes a
local HttpOnly/SameSite operator session. The workspace is at
[localhost:8080](http://127.0.0.1:8080/), with Workspace, Policy and Simulator.
The server also writes a separate restricted `agent-token`; use that path for
[the MCP adapter](agent-integration.md). Tokens, local state and configuration
remain ignored by Git.

## Run the live case

The work order updates payment reconciliation across Engineering and Fraud &
Risk in the same bank. The Risk investigation is restricted; a separate shared
specification is available for implementation.

```sh
./target/debug/trustgraph run --include-risk \
  --goal "$(cat examples/bankcase/work-order.txt)"
```

Risk-context publication to Engineering blocks even if actual tests pass. Use
**Generate from shared sources** for a fresh run excluding the investigation and
old patch. Inspect and approve its exact output. Revising reruns actual tests and
invalidates the old approval. Review again, then **Publish draft PR** performs a
real GitHub operation. The gateway records the receipt in HydraDB. Nothing merges
automatically. The owner can admit additional work orders through the UI; agent
intake accepts only predefined or exactly admitted goals in department mode.

## Qualification and limits

A recorded run on 14 November 2026 exercised real Codex generation, hosted Hydra,
restricted-source denial, fresh repair, stale approval after revision and exact
publication bytes. It created [draft PR #1](https://github.com/skishore23/trustgraph-agent-workbench/pull/1)
at commit `afdaeb442f0ee487157b0a3f7795b524bcc04f5e`, with no merge. This is
historical qualification, not a check of the current external account state.

The new live MCP adapter is tested through actual MCP initialization, discovery
and tool calls over stdio against a mock HTTP backend, including a lost mutation
reply. Those tests do not re-run Codex, Hydra or GitHub. The no-service story is
separately tested through its full policy/release/revision lifecycle.

Permissions are configured department identities, not enterprise IAM. The local
operator can read restricted sources and admit work orders. There is no SSO,
multitenancy, remote hosted-agent deployment or Composio adapter. The runner
supports `src/lib.rs` and `README.md`; dependency installation and arbitrary build
scripts are unsupported. Test execution restricts filesystem and network access.
These controls establish modeled provenance/access and exact review, not an
absolute guarantee of correct code or absence of secrets. See
[the architecture boundary](architecture.md#scope).

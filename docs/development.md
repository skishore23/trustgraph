# Build and live setup

Run commands from the repository root. For the component and data-flow diagrams,
see [architecture](architecture.md); for a service-free introduction, use the
[guided demo](demo.md).

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

Compile the typed fixture plan or save and replay a virtual campaign:

```sh
cargo run --locked -p trustgraph-cli -- compile-plan examples/launch-plan.json
cargo run --locked -p trustgraph-cli -- simulate --seed 0 --count 64 --output /tmp/trustgraph-campaign.json
cargo run --locked -p trustgraph-cli -- replay /tmp/trustgraph-campaign.json
```

`check-model --broken-rule` deliberately finds a stale-approval counterexample
within the finite model. These checks cover the declared bounds and scenarios.

## Start the live workspace

Requirements:

- Rust 1.90 or newer, managed by `rustup` for the live check runner.
- Authenticated `codex`, `hydradb` and `gh` CLIs.
- macOS with `sandbox-exec` for isolated Rust checks.
- A dedicated private personal GitHub repository with a `main` branch and the
  signed-in owner as its sole collaborator.

The publisher verifies repository privacy, collaborators and the base commit.
The runner executes a small Rust workbench with a direct `rustc --test` harness.

Copy the [configuration template](../trustgraph.example.json), then replace the
absolute paths and GitHub repository name in the local copy:

```sh
cp trustgraph.example.json trustgraph.local.json
```

`trustgraph.local.json` is ignored by Git. Keep machine-specific paths and
configuration in that file; the checked-in template contains only placeholders.

The target must be a clean Git checkout of that private repository with committed
`src/lib.rs`, `README.md` and `Cargo.toml` files. The Rust fixture and its tests
belong in `src/lib.rs`. Source imports read **committed Git bytes** from
`context_repository`. Commit controlled inputs before importing them. Use a new
state directory for a different repository or audience; only one server may own
it. Department-mode evidence uses the `bank-departments` namespace.

The hosted Hydra adapter reads the authenticated CLI configuration from
`~/.hydradb/config.json`, or the path in `TRUSTGRAPH_HYDRA_CONFIG`. It requires
an API key and database and currently accepts the official hosted service.

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

## Validation and limits

The live MCP adapter is tested through actual MCP initialization, discovery
and tool calls over stdio against a mock HTTP backend, including a lost mutation
reply. Those tests do not re-run Codex, Hydra or GitHub. The no-service story is
separately tested through its full policy/release/revision lifecycle.

The operator can read restricted sources and admit work orders. Live changes
are limited to `src/lib.rs` and `README.md`; test execution restricts filesystem
and network access. For configured identity boundaries and execution limits, see
[scope](architecture.md#scope). For uncertain publication outcomes and receipt
synchronization, see [recovery](architecture.md#storage-publication-and-recovery).

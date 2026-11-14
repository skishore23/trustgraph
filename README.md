# TrustGraph

TrustGraph controls how AI-generated work is shared. It records the inputs an
agent uses, carries their access restrictions into the result, and requires
human approval for the exact version and destination being published.

For example, a dashboard built from private HR salary data inherits that data's
access restrictions. Sharing a version with Engineering requires a fresh run
using separately shared planning data, followed by review of the exact output.
Passing tests or approving restricted work cannot grant source access.

## Try the guided demo

Requires Rust 1.90 or newer. Run commands from the repository root:

```sh
cargo run --locked -p trustgraph-cli -- story
```

Open [localhost:9081](http://127.0.0.1:9081/) and follow **Company → Policy →
Simulator → Enforce → Revision**. The demo uses fictional data, a fixed clock
and in-memory state. It needs no sign-in or external services and records
releases locally. See the [demo walkthrough](docs/demo.md).

## Architecture

TrustGraph combines three responsibilities:

1. **Capture inputs.** The live gateway starts a fresh Codex run and records its
   supplied sources, instructions, output and actual Rust test results.
2. **Check access and approval.** Rust follows the recorded dependencies,
   intersects source reader sets and verifies human approval of the exact output,
   destination and policy context.
3. **Control publication.** The coordinator rechecks evidence, journals the
   attempt and sends the checked files through the GitHub publisher.

HydraDB stores connected evidence. A durable local journal tracks runtime state
and publication outcomes. Read the [architecture guide](docs/architecture.md)
for Mermaid component, sequence, evidence-flow and recovery diagrams.

## Run the live integration

The live gateway uses authenticated Codex, HydraDB and GitHub integrations,
macOS for isolated Rust checks, and a dedicated private personal repository.
Follow [live setup](docs/development.md#start-the-live-workspace) to configure
[trustgraph.example.json](trustgraph.example.json), then run:

```sh
cargo build --locked -p trustgraph-cli
./target/debug/trustgraph start
```

Open the private operator launch file printed at startup. The operator reviews
and approves the exact change before creating a **draft pull request**.
See [agent integration](docs/agent-integration.md) for MCP and HTTP clients, and
[scope](docs/architecture.md#scope) for the current execution boundary.

## Repository layout

| Path | Purpose |
| --- | --- |
| `crates/trustgraph-core` | Deterministic verification, lineage and approval binding |
| `crates/trustgraph-runtime` | Policy compilation, coordination, journals and recovery |
| `crates/trustgraph-live` | Codex execution, hosted Hydra evidence, HTTP API and GitHub publication |
| `crates/trustgraph-cli` | Commands, restricted MCP adapter and guided demo server |
| `crates/trustgraph-sim` | Bounded model checks and reproducible simulation campaigns |
| `web` | Embedded guided-demo and live-workspace assets |
| `examples` | Policy examples, simulation inputs and fictional bank source material |
| `docs` | Architecture, setup and usage guides |
| `media` | Current explainers and captions |

## Development

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

See [development checks](docs/development.md#local-checks) for model exploration
and campaign replay. Local configuration, runtime state, credentials and build
output are ignored by Git.

## Documentation

| Guide | What it explains |
| --- | --- |
| [Architecture](docs/architecture.md) | Components, captured evidence, inherited access, approval and recovery |
| [Policies and simulation](docs/policy.md) | Company YAML, live JSON and hypothetical policy comparisons |
| [Agent integration](docs/agent-integration.md) | Restricted MCP tools, HTTP requests and operator authority |
| [Demo walkthrough](docs/demo.md) | The complete fictional dashboard scenario |
| [Development and live setup](docs/development.md) | Configuration, checks and the real bank workbench |

Watch the [technical explainer](media/trustgraph-technical-explainer.mp4)
([captions](media/trustgraph-technical-explainer.srt)) or the
[story explainer](media/trustgraph-explainer.mp4)
([captions](media/trustgraph-explainer.srt)).

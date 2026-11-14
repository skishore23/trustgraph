# TrustGraph

TrustGraph controls how AI-generated work is shared. It records the inputs an
agent uses, carries their access restrictions into the result, and requires
human approval for the exact version being published.

For example, a dashboard built from private HR salary data inherits that data’s
access restrictions. Sharing it with Engineering requires a fresh run using
separately shared planning data, followed by review of the exact output.

## Try the demo

Requires Rust 1.90 or newer.

```sh
cargo run --locked -p trustgraph-cli -- story
```

Open [localhost:9081](http://127.0.0.1:9081/) and follow **Company → Policy →
Simulator → Enforce → Revision**. The demo uses fictional data and local,
in-memory state; no sign-in or external services are needed.

## How it works

1. An agent submits a task through the gateway.
2. TrustGraph starts a controlled run and records its inputs and output.
3. The Rust verifier checks input history and who may access the result.
4. A human reviews the exact output and destination.
5. The runtime rechecks authorization and publishes through a controlled adapter.

HydraDB stores the evidence and its relationships. Rust evaluates policy. A
durable local journal tracks publication and recovery.

## Use the live integration

The live gateway uses Codex, HydraDB, and GitHub to create a **draft pull request**.
Follow [setup](docs/development.md) to configure those services using
[trustgraph.example.json](trustgraph.example.json), then run:

```sh
cargo build --locked -p trustgraph-cli
./target/debug/trustgraph start
```

See [agent integration](docs/agent-integration.md) for MCP and HTTP access.

## Development

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

## Documentation

- [Architecture](docs/architecture.md)
- [Policies and simulation](docs/policy.md)
- [Demo walkthrough](docs/demo.md)
- [Setup, checks, and current limits](docs/development.md)
- [Technical explainer](media/trustgraph-technical-explainer.mp4) ([captions](media/trustgraph-technical-explainer.srt))
- [Story explainer](media/trustgraph-explainer.mp4) ([captions](media/trustgraph-explainer.srt))

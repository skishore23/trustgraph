# Development

Requires Rust 1.90 or newer. The site tests also need Node.js. Run commands
from the repository root.

## Local checks

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

The site's page logic and static build have their own tests:

```sh
node web/site.test.cjs
node scripts/test_build_demo.cjs
```

To run the bounded proofs, see [verification](verification.md#reproduce).

## Run the site locally

The playground page and `POST /api/policy`, without Vercel credentials. Files
are read from `web/` on each request, so edits show on refresh.

```sh
cargo run --release --locked --example site_server
# http://127.0.0.1:9082
```

## Run `trustgraph serve` locally

Use a state directory outside the repository:

```sh
cargo build --locked -p trustgraph-cli
./target/debug/trustgraph policy init --path /tmp/tg/trustgraph.policy.yaml
./target/debug/trustgraph serve --policy /tmp/tg/trustgraph.policy.yaml --operator leo --state /tmp/tg/state
python3 examples/python_client.py --tokens /tmp/tg/state/tokens
```

Everything else about the service is in [serve](serve.md).

## Where things live

| Path | What it is |
| --- | --- |
| `crates/trustgraph-policy` | The policy file format; `starter.policy.yaml` is what `policy init` writes |
| `crates/trustgraph-engine` | The journaled runtime; its `fault-injection` feature is for the simulator only |
| `crates/trustgraph-core` | The decision and reader-set kernels and their Kani harnesses |
| `crates/trustgraph-framework` | The lineage graph the engine is built on |
| `crates/trustgraph-sim` | The policy simulator |
| `crates/trustgraph-cli` | `trustgraph policy` (`src/policy_cmd.rs`) and `trustgraph serve` (`src/serve/`) |
| `web/serve.*` | The operator page, compiled into the binary |
| `web/index.html`, `web/site.*` | The public site |
| `api/policy.rs`, `hosted/` | The site's `POST /api/policy` function; see [the public site](vercel.md) |
| `examples/policies` | Five worked policies, also checked by the policy crate's tests |

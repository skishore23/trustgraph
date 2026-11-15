# Composio GitHub issues example

An 8090 native agent proposes a GitHub issue; a human operator approves the
exact action in TrustGraph; only then does the gateway execute it through
Composio. This is an integration example, not part of the TrustGraph core.

The pattern: your tool layer calls TrustGraph before it acts.

## Run the preview

Preview mode records a simulated receipt and creates no GitHub issue. From this
directory:

```sh
cp trustgraph-actions.example.json trustgraph-actions.local.json
cargo run -p trustgraph-composio-example --
```

Flags: `--config FILE` (default `trustgraph-actions.local.json`), `--port 8084`,
`--agent-port 8085`, `--check` (verify Composio Connect credentials without
writing). The local config and `.trustgraph-live/` state are git-ignored.

## Docs

- [8090 to TrustGraph to GitHub issue](docs/8090-issue-actions.md)
- [Delegate an 8090 work order to TrustGraph](docs/8090-demo.md)
- [One-hour live issue demo](docs/one-hour-issue-demo.md)

## Tests

```sh
cargo test -p trustgraph-composio-example
python3 scripts/test_actions.py
python3 scripts/test_8090_demo.py
node scripts/test_walkthrough.cjs
```

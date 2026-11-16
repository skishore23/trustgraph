# Gateway hand-off

Two agents reach a file store and a chat only through `trustgraph gateway`.
The researcher reads the salary file and hands off; the planner receives,
reads the roadmap and tries to post to #engineering. The gateway refuses the
post and never forwards it. The upstreams are a tiny fake MCP server
(`crates/trustgraph-cli/examples/fake_upstream.rs`), so nothing external runs.

- [gateway.yaml](gateway.yaml): which tool reads which source, which tool is which action.
- [handoff.policy.yaml](../policies/handoff.policy.yaml): the policy.

Run it from the repository root:

```sh
cargo build -p trustgraph-cli --bins --examples
SLACK_BOT_TOKEN=demo-secret target/debug/trustgraph gateway \
  --policy examples/policies/handoff.policy.yaml --operator nina \
  --config examples/gateway/gateway.yaml --state ~/tg-gateway
# in another terminal:
target/debug/examples/gateway_demo --tokens ~/tg-gateway/tokens
```

```text
researcher drive__read_file HR/Compensation/salaries.csv: nina: 120k, sam: 95k
researcher trustgraph__handoff: a hand-off token
planner trustgraph__receive: {"carries":["salaries"],"received":true}
planner drive__read_file Product/roadmap.md: Q3: ship the new billing page
planner slack__post_message #engineering: error: TrustGraph gateway: `post_to_engineering` was not forwarded: denied: `salaries` is readable by hr only; engineering (ravi, sam) is not included (denied)
```

`trustgraph gateway check --config examples/gateway/gateway.yaml --policy
examples/policies/handoff.policy.yaml` says the same in plain English. See
[docs/gateway.md](../../docs/gateway.md).

# Gateway hand-off

Two agents reach a file store and a chat only through `trustgraph gateway`.
The researcher reads the salary file and hands off; the planner receives,
reads the roadmap and tries to post to #engineering. The gateway refuses the
post and never forwards it. The planner then tries to slip an extra argument
(`Channel`) past the check; the gateway refuses that too, because a call may
carry only the arguments its tool lists. The upstreams are a tiny fake MCP
server (`crates/trustgraph-cli/examples/fake_upstream.rs`), so nothing
external runs.

- [gateway.yaml](gateway.yaml): which tool reads which source, which tool is
  which action, and which arguments each forwards.
- [gateway.policy.yaml](gateway.policy.yaml): the hand-off policy plus an
  `unmatched` source, the most restrictive one, for a drive path no rule names.

Run it from the repository root:

```sh
cargo build -p trustgraph-cli --bins --examples
SLACK_BOT_TOKEN=demo-slack-token-0001 target/debug/trustgraph gateway \
  --policy examples/gateway/gateway.policy.yaml --operator nina \
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
planner slack__post_message #engineering, plus Channel #all-company: error: TrustGraph gateway: `Channel` is not an argument the gateway forwards for this tool (it forwards: channel, text); the call was not forwarded (invalid_arguments)
```

The gateway starts with one warning, which is expected here:

```text
warning: tools.slack.post_message.arguments: not constrained by any `when`: text; each is hashed into the checked output and shown to the approver, but can hold anything, so it must not be able to choose who the call reaches
```

`text` is the message; `channel` decides who the post reaches, and its rule
matches it exactly (`when` values in `acts` rules match exactly unless the
rule asks for `type: email`, `glob` or `path`). Stop the gateway with Ctrl-C:
it stops both fake upstreams and removes their private `HOME` directories.

`SLACK_BOT_TOKEN=demo-slack-token-0001 trustgraph gateway check --config
examples/gateway/gateway.yaml --policy examples/gateway/gateway.policy.yaml`
says the same in plain English, including the upstream arguments the config
does not forward (`blocks, thread`). It runs the fake upstream to ask for its
tools; add `--offline` to check only the config and policy. See
[docs/gateway.md](../../docs/gateway.md).

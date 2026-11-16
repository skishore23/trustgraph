# MCP proxy hand-off

Each agent reaches the demo file server through its own
`trustgraph-mcp-proxy`. The researcher's proxy writes a hand-off file; the
planner's proxy starts from it, so the planner's post is blocked.

The setup: a mapping file and one line in the MCP client's configuration.

```yaml
# mapping.yaml
sources:
  read_file:
    - { source: salaries, when: { path: "hr/*" } }
    - { source: roadmap, when: { path: "product/*" } }
actions:
  post_message: { action: post_to_engineering, content: text }
unmapped: block
```

```json
{ "mcpServers": { "files": {
    "command": "trustgraph-mcp-proxy",
    "args": ["--map", "mapping.yaml", "--token-file", "/path/to/planner.token",
             "--", "python", "files_server.py"] } } }
```

Run it:

```sh
pip install "./sdk/python[mcp]"
trustgraph serve --policy examples/policies/handoff.policy.yaml --operator nina --state ~/tg-handoff
python examples/integrations/mcp/handoff.py --tokens ~/tg-handoff/tokens
```

```text
researcher read_file hr/salaries.csv: nina: 120k, sam: 95k
planner read_file product/roadmap.md: Q3: ship the new billing page
planner post_message: error: TrustGraph blocked `post_to_engineering`: denied: `salaries` is readable by hr only; engineering (ravi, sam) is not included. The tool did not run.
```

See [docs/integrations.md](../../../docs/integrations.md#mcp-proxy).

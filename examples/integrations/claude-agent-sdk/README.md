# Claude Agent SDK subagent hand-off

A researcher subagent reads the salary file and returns; its parent, the
planner, reads the roadmap and tries to post to engineering. The post is
denied, because the subagent's session returns to the parent when it stops.

The Claude Code CLI would call a model, so the script plays the CLI's side of
the SDK's control protocol; the hooks run in the real `ClaudeSDKClient`.

The ten lines that matter:

```python
from trustgraph.integrations.claude_agent_sdk import Guard

guard = Guard(
    trustgraph.Client(token_file=".../planner.token"),
    sources={"mcp__drive__salaries": "salaries", "mcp__drive__roadmap": "roadmap"},
    actions={"mcp__slack__post": "post_to_engineering"},
    subagents={"researcher": trustgraph.Client(token_file=".../researcher.token")},
)
options = ClaudeAgentOptions(hooks=guard.hooks(), mcp_servers=...)
async with ClaudeSDKClient(options) as agent:
    ...
```

Run it:

```sh
pip install "./sdk/python[claude-agent-sdk]"
trustgraph serve --policy examples/policies/handoff.policy.yaml --operator nina --state ~/tg-handoff
python examples/integrations/claude-agent-sdk/subagent.py --tokens ~/tg-handoff/tokens
```

```text
researcher subagent read salaries and returned its notes
planner mcp__slack__post: deny: TrustGraph blocked `post_to_engineering`: denied: `salaries` is readable by hr only; engineering (ravi, sam) is not included. The tool did not run.
```

See [docs/integrations.md](../../../docs/integrations.md#claude-agent-sdk).

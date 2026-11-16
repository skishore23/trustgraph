# OpenAI Agents SDK hand-off

A researcher agent reads the salary file and hands off to a planner, which
reads the roadmap and tries to post to engineering. The post is blocked,
because the hand-off carries `salaries`. The models are the SDK's
`ScriptedModel`, so no model API is called.

The ten lines that matter:

```python
from trustgraph.integrations.openai_agents import Guard

guard = Guard(
    clients={"researcher": trustgraph.Client(token_file=".../researcher.token"),
             "planner": trustgraph.Client(token_file=".../planner.token")},
    sources={"read_salaries": "salaries", "read_roadmap": "roadmap"},
    actions={"post_to_engineering": "post_to_engineering"},
)
guard.protect(researcher, planner)
result = guard.run_sync(researcher, "Plan Q3 and tell engineering.")
```

Run it:

```sh
pip install "./sdk/python[openai-agents]"
trustgraph serve --policy examples/policies/handoff.policy.yaml --operator nina --state ~/tg-handoff
python examples/integrations/openai-agents/handoff.py --tokens ~/tg-handoff/tokens
```

```text
researcher read_salaries: nina: 120k, sam: 95k
planner read_roadmap: Q3: ship the new billing page
planner post_to_engineering: TrustGraph blocked `post_to_engineering`: denied: `salaries` is readable by hr only; engineering (ravi, sam) is not included. The tool did not run.
```

See [docs/integrations.md](../../../docs/integrations.md#openai-agents-sdk).

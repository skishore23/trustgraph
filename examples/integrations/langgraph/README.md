# LangGraph hand-off

A researcher agent reads the salary file; a planner agent receives its work,
reads the roadmap and tries to post to engineering. The post is blocked,
because what the planner received carries `salaries`. The models are scripted,
so no model API is called.

The ten lines that matter:

```python
from trustgraph.integrations.langchain import Guard, agent_node

guard = Guard(sources={"read_salaries": "salaries", "read_roadmap": "roadmap"},
              actions={"post_to_engineering": "post_to_engineering"})
researcher = create_agent(model, tools, middleware=[guard.middleware()])
planner = create_agent(model, tools, middleware=[guard.middleware()])

graph = StateGraph(State)  # State declares `trustgraph: str`, the hand-off token
graph.add_node("researcher", agent_node(trustgraph.Client(token_file=".../researcher.token"), researcher, first=True))
graph.add_node("planner", agent_node(trustgraph.Client(token_file=".../planner.token"), planner))
...
graph.compile().invoke({"messages": [...], "trustgraph": ""})
```

Run it:

```sh
pip install "./sdk/python[langchain]"
trustgraph serve --policy examples/policies/handoff.policy.yaml --operator nina --state ~/tg-handoff
python examples/integrations/langgraph/handoff.py --tokens ~/tg-handoff/tokens
```

```text
read_salaries: [success] nina: 120k, sam: 95k
read_roadmap: [success] Q3: ship the new billing page
post_to_engineering: [error] TrustGraph blocked `post_to_engineering`: denied: `salaries` is readable by hr only; engineering (ravi, sam) is not included. The tool did not run.
```

See [docs/integrations.md](../../../docs/integrations.md#langchain-and-langgraph).

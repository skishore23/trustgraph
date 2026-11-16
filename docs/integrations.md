# Integrations

Each integration connects an agent framework to `trustgraph serve` through
the [Python SDK](sdk-python.md). A **session** belongs to one agent's run and
remembers everything the agent read. A tool marked as a **source** registers
its result as that policy source in the session. A tool marked as an
**action** is checked before it runs, as an output of everything the session
has read; if it is not allowed, it does not run and the agent gets the reason
as the tool's result. A **hand-off** passes a token carrying what one agent's
session read to the next agent's session, so restrictions follow the work.

| Framework | How it plugs in | Can the agent bypass it? |
| --- | --- | --- |
| [LangChain / LangGraph](#langchain-and-langgraph) | Tool wrappers, or `ToolNode` `wrap_tool_call` / `create_agent` middleware | Only through tools you did not wrap or map |

A library integration is only as strong as the host program: a tool called
without the wrapper, or a tool missing from the map, is not checked. Sessions
over-restrict by design (see [Sessions](sdk-python.md#sessions)).

## LangChain and LangGraph

Tested with langchain 1.5.0, langchain-core 1.8.1 and langgraph 1.2.22.

```sh
pip install "./sdk/python[langchain]"
```

```python
import trustgraph
from trustgraph.integrations.langchain import Guard, as_action, as_source

client = trustgraph.Client(token_file="~/.trustgraph/serve/tokens/planner.token")
guard = Guard(sources={"read_salaries": "salaries"}, actions={"post_message": "post_to_engineering"})
agent = create_agent(model, [read_salaries, post_message], middleware=[guard.middleware()])

with client.session():
    agent.invoke({"messages": [("user", "Post the Q3 plan")]})
```

- **Tools.** `as_source(tool, "salaries")` and `as_action(tool, "post_to_engineering")`
  return a wrapped copy of any `BaseTool`, including `@tool` functions, sync
  or async. `Action("post_to_engineering", content=lambda args: args["text"],
  recipients=lambda args: args["to"])` picks what is recorded and who receives
  it; by default all arguments are recorded and the action's whole audience
  is checked.
- **Tool nodes.** `Guard(sources=..., actions=...)` maps tool names at the
  tool-execution step: `ToolNode(tools, wrap_tool_call=guard.wrap_tool_call,
  awrap_tool_call=guard.awrap_tool_call)`, or `middleware=[guard.middleware()]`
  for `create_agent`. Tools in neither map run untouched.
- **Blocked calls.** The tool does not run; the agent receives a `ToolMessage`
  with `status="error"`: "TrustGraph blocked …" for a deny, "TrustGraph held …"
  with the decision id for needs-approval, and "could not complete the check"
  when the server cannot be reached. A source result that cannot be registered
  is withheld.
- **Sessions.** The current session (`with client.session():` around the run).
  With no session, `Guard(..., client=client)` keeps one session per LangGraph
  `thread_id` for the life of the guard; otherwise mapped tools are refused.
- **Hand-offs.** `agent_node(client, agent)` makes a graph node that runs a
  sub-agent in a new session of that client's agent, starting from the
  hand-off token in `state["trustgraph"]` and writing its own token back.
  If the receiving agent is not cleared for what the token carries, the node
  raises and the run stops. Example:
  [examples/integrations/langgraph](../examples/integrations/langgraph).

Not covered: tools that return a `Command` cannot be sources (the result is
withheld); tool calls a graph makes outside `ToolNode` or `create_agent` are
not seen by the guard; the hand-off token lives in graph state, so any node
that rewrites `state["trustgraph"]` can drop restrictions.

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
| [OpenAI Agents SDK](#openai-agents-sdk) | Function-tool input and output guardrails; run hooks for hand-offs | Only through tools you did not map, or tool kinds guardrails do not cover |
| [Claude Agent SDK](#claude-agent-sdk) | `PreToolUse` / `PostToolUse` / `SubagentStart` / `SubagentStop` hooks | Only through tools you did not map; hooks see every tool call, built-in and MCP |
| [MCP: `trustgraph gateway`](#mcp) | One MCP endpoint between any MCP client and its MCP servers (Rust, not the SDK) | Not if the gateway alone holds the upstreams' credentials |

In place of a blocked tool's result, every SDK integration gives the agent one of
three texts: "TrustGraph blocked `<action>`: denied: …" for a deny,
"TrustGraph held `<action>`: needs approval: … (decision <id>)" for needs
approval, or "TrustGraph could not complete the check …" when the server
cannot be reached or refuses. In each case the tool did not run. A source
result that cannot be registered is withheld from the agent.

A library integration is only as strong as the host program: a tool called
without the wrapper, or a tool missing from the map, is not checked. The
gateway is the option an agent cannot skip, provided only the gateway can
reach the upstream servers. Sessions over-restrict by design (see
[Sessions](sdk-python.md#sessions)).

## LangChain and LangGraph

Tested with langchain 1.5.0, langchain-core 1.8.1 and langgraph 1.2.22.

```sh
pip install "./sdk/python[langchain]"
```

```python
import trustgraph
from trustgraph.integrations.langchain import Guard

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
- **Blocked calls** come back as a `ToolMessage` with `status="error"`.
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

## OpenAI Agents SDK

Tested with openai-agents 0.24.0.

```sh
pip install "./sdk/python[openai-agents]"
```

```python
import trustgraph
from trustgraph.integrations.openai_agents import Guard

guard = Guard(
    clients={"planner": trustgraph.Client(token_file="~/.trustgraph/serve/tokens/planner.token")},
    sources={"read_salaries": "salaries"},
    actions={"post_message": "post_to_engineering"},
)
guard.protect(planner)  # planner = Agent(name="planner", tools=[read_salaries, post_message], ...)
result = guard.run_sync(planner, "Post the Q3 plan")
```

- **Tools.** An action is a tool input guardrail: it records the call's
  arguments (or `Action(..., content=lambda args: ...)`) and checks them before
  the tool runs. A source is a tool output guardrail: it registers the result
  before the model sees it. `as_source(tool, ...)` and `as_action(tool, ...)`
  return a guarded copy of one `FunctionTool`; `guard.protect(*agents)` does
  this for every mapped tool of those agents.
- **Blocked calls** are the guardrail's `reject_content`.
- **Sessions.** `guard.run` / `guard.run_sync` give each agent of the run its
  own session, from its entry in `clients`; an agent with no client is
  refused. Without `Guard.run`, the guardrails use the current session
  (`with client.session():`). For `Runner.run_streamed`, wrap the call in
  `with guard.scope():` and pass `hooks=guard.hooks()`.
- **Hand-offs.** On every hand-off the receiving agent's session receives what
  the previous agent's session saw (`RunHooks.on_handoff`). If the receiving
  agent is not cleared for it, the run stops with `TrustGraphError`.
  Example: [examples/integrations/openai-agents](../examples/integrations/openai-agents).

Not covered: hosted tools (web search, file search, hosted MCP, code
interpreter, image generation), computer, shell and apply-patch tools, and
the hand-off call itself do not go through tool guardrails. `Agent.as_tool()`
runs a nested agent whose session is not linked to the caller's: map the
agent-tool itself as a source if what it returns must stay restricted.

## Claude Agent SDK

Tested with claude-agent-sdk 0.2.182.

```sh
pip install "./sdk/python[claude-agent-sdk]"
```

```python
import trustgraph
from claude_agent_sdk import ClaudeAgentOptions, ClaudeSDKClient
from trustgraph.integrations.claude_agent_sdk import Guard

guard = Guard(
    trustgraph.Client(token_file="~/.trustgraph/serve/tokens/planner.token"),
    sources={"mcp__drive__read": "salaries"},
    actions={"mcp__slack__post": "post_to_engineering"},
)
async with ClaudeSDKClient(ClaudeAgentOptions(hooks=guard.hooks(), mcp_servers=servers)) as agent:
    await agent.query("Post the Q3 plan")
```

- **Tools.** Map exact tool names: built-in (`Read`, `WebFetch`, …), MCP
  (`mcp__<server>__<tool>`) and in-process SDK tools alike. A mapped action is
  recorded with its `tool_input` (or `Action(..., content=lambda args: ...)`)
  and checked in `PreToolUse`; a mapped source's `tool_response` is
  registered in `PostToolUse`.
- **Blocked calls** are `permissionDecision: "deny"` with the text as the
  reason. Needs approval is a deny too, not "ask": the SDK's own permission
  prompt would let someone approve around the operator.
- **Fail closed.** Every matcher is `on_failure="block"`, so a hook that raises
  or times out blocks the tool. The SDK keeps a tool's result when a
  `PostToolUse` hook fails, so the source hook never raises: if the result
  cannot be registered, it replaces the result (`updatedToolOutput`) and stops
  the run (`continue: false`). Keep the client's timeout (30 s) below the hook
  timeout (`Guard(timeout=60)`).
- **Sessions.** One per SDK `session_id`, created on first use; `guard.forget(session_id)`
  drops it. `guard.session(session_id).handoff()` hands it to another process.
- **Subagents.** Each subagent (`agent_id`) gets its own session, from
  `subagents={agent_type: client}` or the main client. It starts with what its
  parent had seen, because its prompt was written by the parent, and when it
  stops the parent receives what it saw. For a subagent not cleared for its
  parent's sources the `SubagentStart` hook fails, which with
  `on_failure="block"` keeps it from starting, and its mapped tools are
  refused either way.
  Example: [examples/integrations/claude-agent-sdk](../examples/integrations/claude-agent-sdk).

Not covered: the tests drive the real `ClaudeSDKClient` hook dispatch with a
scripted CLI on the other end, not the Claude Code CLI itself. Unmapped tools,
including `Bash`, run untouched: a shell command can read a source or post
somewhere without a check.

## MCP

For MCP servers, use [`trustgraph gateway`](gateway.md) instead of a
library. It is part of the `trustgraph` binary, serves one MCP endpoint to
every agent, holds the upstream servers' credentials, registers what listed
tools return and checks listed actions before forwarding them. Hand-offs are
two tools of its own. Example: [examples/gateway](../examples/gateway).

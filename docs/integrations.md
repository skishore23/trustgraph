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
| [Claude Agent SDK](#claude-agent-sdk) | `PreToolUse` / `PostToolUse` / `SubagentStart` / `SubagentStop` / `UserPromptSubmit` hooks | Only through tools you did not map; hooks see every tool call, built-in and MCP |
| [MCP: `trustgraph gateway`](#mcp) | One MCP endpoint between any MCP client and its MCP servers (Rust, not the SDK) | Not if the gateway alone holds the upstreams' credentials |

The texts the agent gets, and every case that fails closed, are in
[Exceptions and failing closed](sdk-python.md#exceptions-and-failing-closed);
what each integration does about resumed conversations is in
[Sessions](sdk-python.md#sessions).

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
  or async. A wrapped tool called from inside another wrapped tool is checked
  too. `Action("post_to_engineering", content=lambda args: args["text"],
  recipients=lambda args: args["to"])` picks what is recorded and who receives
  it; by default all arguments are recorded and the action's whole audience
  is checked.
- **Tool nodes.** `Guard(sources=..., actions=...)` maps tool names at the
  tool-execution step: `ToolNode(tools, wrap_tool_call=guard.wrap_tool_call,
  awrap_tool_call=guard.awrap_tool_call)`, or `middleware=[guard.middleware()]`
  for `create_agent`. Put the guard last in the middleware list (innermost):
  it checks the arguments and recipients as they are when it runs, so a
  middleware inside it could change them afterwards. Tools in neither map run
  untouched.
- **Blocked calls** come back as a `ToolMessage` with `status="error"`.
- **Sessions.** The current session (`with client.session():` around the run).
  With no session, `Guard(..., client=client)` keeps one session per LangGraph
  `thread_id`, at most `max_threads` (1024) in memory. It restores a thread's
  session from `state["trustgraph"]` and writes the token there after every
  registered result; `guard.middleware()` declares that key, and a `ToolNode`
  graph must declare `trustgraph: str` in its state for the token to be kept.
  Either way, a mapped call is refused (`unrecorded_history`) when the
  conversation holds a mapped source's result that the session did not record
  and the session carries no token: a checkpointed thread resumed with a new
  session, or a thread evicted from memory.
- **Hand-offs.** `agent_node(client, agent)` makes a graph node that runs a
  sub-agent in a new session of that client's agent, starting from the
  hand-off token in `state["trustgraph"]` and writing its own token back.
  Declare `trustgraph: str` in the graph's state and start the graph with
  `{"trustgraph": ""}`: LangGraph silently drops undeclared keys, so a node
  raises `handoff_missing`, before its agent runs, when the key is absent.
  Mark the entry node `agent_node(..., first=True)`; it alone may start from
  an empty token, and not on a thread that already holds model or tool
  messages (`resumed_without_session`). Every other node requires a valid
  token. If the receiving agent is not cleared for what the token carries,
  the node raises and the run stops. On a checkpointed thread, invoke later
  turns without `"trustgraph"` so the checkpointed token is kept. Example:
  [examples/integrations/langgraph](../examples/integrations/langgraph).

Not covered: tools that return a `Command` cannot be sources (the result is
withheld); tool calls a graph makes outside `ToolNode` or `create_agent` are
not seen by the guard; a node that replaces `state["trustgraph"]` with
another valid token drops the restrictions the old one carried. A
`Guard`-mapped tool runs to completion before the guard registers or
withholds its result, so callback handlers, `astream_events` and tracing
integrations (such as LangSmith) receive the raw result first. Resumed
conversations are detected only from the mapped source results still in the
state's messages.

## OpenAI Agents SDK

Tested with openai-agents 0.24.0.

```sh
pip install "./sdk/python[openai-agents]"
```

```python
import trustgraph
from agents import set_tracing_disabled
from trustgraph.integrations.openai_agents import Guard

set_tracing_disabled(True)  # tracing is on by default and exports tool inputs and outputs to OpenAI

guard = Guard(
    clients={"planner": trustgraph.Client(token_file="~/.trustgraph/serve/tokens/planner.token")},
    sources={"read_salaries": "salaries"},
    actions={"post_message": "post_to_engineering"},
)
guard.protect(planner)  # planner = Agent(name="planner", tools=[read_salaries, post_message], ...)
result = guard.run_sync(planner, "Post the Q3 plan")
```

- **Tracing.** The SDK's tracing is on by default and sends every span,
  including tool inputs and outputs (the source content itself), to OpenAI.
  `set_tracing_disabled(True)`, as above, turns it off; so does
  `OPENAI_AGENTS_DISABLE_TRACING=1`.
- **Tools.** An action is a tool input guardrail: it records the call's
  arguments (or `Action(..., content=lambda args: ...)`) and checks them before
  the tool runs. A source is a tool output guardrail: it registers the result
  before the model sees it. `as_source(tool, ...)` and `as_action(tool, ...)`
  return a guarded copy of one `FunctionTool`; `guard.protect(*agents)` does
  this for every mapped tool of those agents, and adds the same checks to
  every server in their `mcp_servers` through the SDK's per-server tool
  guardrails (map an MCP tool by the name the model calls).
- **Blocked calls** are the guardrail's `reject_content`.
- **Sessions.** `guard.run` / `guard.run_sync` / `await guard.run_streamed`
  give each agent of the run its own session, from its entry in `clients`; an
  agent with no client is refused. Pass a dict as `context=` and the run's
  token is kept in `context["trustgraph"]`: every session of a later run given
  that context, or resumed from a `RunState`, starts carrying it. A run that
  continues a conversation without it is refused. Without `Guard.run`, the
  guardrails use the current session (`with client.session():`) as it is.
  To call `Runner` yourself, wrap it in `with guard.scope(input, **kwargs):`
  and pass `hooks=guard.hooks()`.
- **Hand-offs.** On every hand-off the receiving agent's session receives what
  the previous agent's session saw (`RunHooks.on_handoff`). If the receiving
  agent is not cleared for it, the run stops with `TrustGraphError`.
  Example: [examples/integrations/openai-agents](../examples/integrations/openai-agents).

Not covered: hosted tools (web search, file search, hosted MCP, code
interpreter, image generation), computer, shell and apply-patch tools, and
the hand-off call itself do not go through tool guardrails. Tools and MCP
servers of agents not passed to `protect` are not guarded. `Agent.as_tool()`
runs a nested agent whose session is not linked to the caller's: map the
agent-tool itself as a source if what it returns must stay restricted. A
context that is not a dict cannot keep the token, so a resumed run with one
is refused.

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
- **A read that cannot be recorded.** Every matcher is `on_failure="block"`,
  so a hook that raises or times out blocks the tool. A `PostToolUse` hook
  cannot take a result back: if a source result cannot be registered, the
  hook replaces it (`updatedToolOutput`) and stops the turn (`continue:
  false`), but the original result may still be in the transcript (the SDK
  keeps it, for instance, when the replacement does not fit a built-in
  tool's output). So the SDK session is marked broken: from then on every
  mapped action, subagent start and hand-off in it is refused ("a read could
  not be recorded"), for as long as the session id lives. Keep the client's
  timeout (30 s) below the hook timeout (`Guard(timeout=60)`).
- **Sessions.** One per SDK `session_id`, at most `max_sessions` (1024) in
  memory; `guard.forget(session_id)` drops one from memory.
  `Guard(store=DIR)` keeps each SDK session's token, and whether it is
  broken, in a file under `DIR` (mode 0600), so an SDK session resumed in a
  new process (`resume=`) starts from it. An SDK session the guard has no
  record of, whose transcript (`transcript_path`) already holds tool results,
  is refused: a resume without the store, a `fork_session` (a new id), or a
  session evicted from memory. `guard.session(session_id).handoff()` hands a
  session to another process.
- **Subagents.** Each subagent (`agent_id`) gets its own session, from
  `subagents={agent_type: client}` or the main client. It starts with what its
  parent had seen, because its prompt was written by the parent, and when it
  stops the parent receives what it saw. The hook input names the subagent
  but not the agent that spawned it, so a subagent starts from the main
  session and every subagent still running (one of which may be its parent),
  and on stopping hands back to the same sessions; a `parent_agent_id` in
  the hook input, if the CLI sends one, is used instead. Running subagents
  therefore over-restrict each other. For a subagent not cleared for what it
  starts from, the `SubagentStart` hook fails, which with
  `on_failure="block"` keeps it from starting, and its mapped tools are
  refused either way, as are those of a subagent that did not start under
  the guard.
  Example: [examples/integrations/claude-agent-sdk](../examples/integrations/claude-agent-sdk).

Not covered: the tests drive the real `ClaudeSDKClient` hook dispatch with a
scripted CLI on the other end, not the Claude Code CLI itself; the resume
detection looks for `tool_result` entries in the transcript, in a format the
tests write rather than one observed from the CLI. Unmapped tools, including `Bash`, run untouched: a shell command
can read a source or post somewhere without a check.

## MCP

For MCP servers, use [`trustgraph gateway`](gateway.md) instead of a
library. It is part of the `trustgraph` binary, serves one MCP endpoint to
every agent, holds the upstream servers' credentials, registers what listed
tools return and checks listed actions before forwarding them. Hand-offs are
two tools of its own. Example: [examples/gateway](../examples/gateway).

"""OpenAI Agents SDK.

Sources and actions are function-tool guardrails: an action is checked by a
tool *input* guardrail before the tool runs, and a source is registered by a
tool *output* guardrail before the model sees the result. A blocked call is
``reject_content``: the tool does not run (or its result is withheld) and the
model receives the reason.

``Guard.run`` gives each agent of a run its own session, made from that
agent's ``Client``, and carries what one agent's session has seen to the next
on every hand-off. With a dict run context the run's token is kept in
``context["trustgraph"]`` and restored from it; a run that resumes a
conversation without it is refused.
"""

from __future__ import annotations

import asyncio
import contextlib
import contextvars
import dataclasses
import inspect
import json
import threading
from collections.abc import Iterator, Mapping, MutableMapping
from typing import Any

from agents import (
    Agent,
    FunctionTool,
    RunHooks,
    Runner,
    RunState,
    ToolGuardrailFunctionOutput,
    ToolInputGuardrail,
    ToolInputGuardrailData,
    ToolOutputGuardrail,
    ToolOutputGuardrailData,
)

from .._client import Client
from .._errors import TrustGraphError
from .._session import Session, _token, current_session
from ._common import Action, as_spec, blocked_message, check_action, not_registered_message

__all__ = ["Action", "Guard", "as_action", "as_source"]


CONTEXT_KEY = "trustgraph"  # where a dict run context keeps the run's hand-off token
_RESUMED = (
    "this run continues an earlier conversation (a RunState, session=, previous_response_id, conversation_id, "
    "or earlier model or tool items in the input) but carries no TrustGraph session for it; pass the earlier "
    f"run's dict context, which holds the token under {CONTEXT_KEY!r}"
)


def _resumed(input: Any, kwargs: Mapping[str, Any]) -> bool:
    """Whether the run continues a conversation whose earlier turns this guard did not see."""
    if isinstance(input, RunState):
        return True
    if kwargs.get("previous_response_id") or kwargs.get("conversation_id"):
        return True
    if isinstance(input, list):
        return any(not (isinstance(i, Mapping) and i.get("role") in ("user", "system", "developer")) for i in input)
    return False


class _Run:
    """The sessions of one run, one per agent name.

    With a dict run context, the run's hand-off token (everything any of its
    sessions has seen) is kept in ``context["trustgraph"]`` and every session
    starts carrying it, so a later run given the same context (or a
    ``RunState``, which serialises a dict context) restores it.
    """

    def __init__(self, clients: Mapping[str, Client], resumed: bool = False) -> None:
        self.clients = dict(clients)
        self.sessions: dict[str, Session] = {}
        self.lock = threading.Lock()
        self.resumed = resumed

    def session(self, agent: str, context: Any = None) -> Session:
        with self.lock:
            if agent in self.sessions:
                return self.sessions[agent]
            if agent not in self.clients:
                raise TrustGraphError("no_session", f"no TrustGraph client for agent `{agent}`")
            client = self.clients[agent]
        s = client.session()
        stored = context.get(CONTEXT_KEY) if isinstance(context, Mapping) else None
        if stored:
            try:
                s.receive(stored)  # a failure breaks `s`: its mapped tools are refused
            except TrustGraphError:
                pass
        elif self.resumed:
            s.mark_broken(_RESUMED)
        with self.lock:
            s = self.sessions.setdefault(agent, s)
        self.save(context)
        return s

    def save(self, context: Any) -> None:
        if not isinstance(context, MutableMapping):
            return
        with self.lock:
            ids = list(dict.fromkeys(i for s in self.sessions.values() for i in s.seen))
        context[CONTEXT_KEY] = _token(ids)


_run: contextvars.ContextVar[_Run | None] = contextvars.ContextVar("trustgraph_openai_run", default=None)


async def _session(agent: Agent[Any], wrapper: Any) -> tuple[Session, _Run | None]:
    run = _run.get()
    if run is not None:
        return await asyncio.to_thread(run.session, agent.name, getattr(wrapper, "context", None)), run
    s = current_session()
    if s is None:
        raise TrustGraphError("no_session", "no active TrustGraph session for this run")
    return s, None


def _args(data: ToolInputGuardrailData) -> dict[str, Any]:
    raw = getattr(data.context, "tool_arguments", None) or "{}"
    try:
        args = json.loads(raw)
    except ValueError:
        return {"arguments": raw}
    return args if isinstance(args, dict) else {"arguments": args}


async def _check(spec: Action, data: ToolInputGuardrailData) -> ToolGuardrailFunctionOutput:
    try:
        s, _ = await _session(data.agent, data.context)
        await asyncio.to_thread(check_action, s, spec, _args(data))
    except TrustGraphError as e:
        return ToolGuardrailFunctionOutput.reject_content(blocked_message(e, spec.name), output_info=e.code)
    except Exception as e:  # fail closed on anything unexpected
        err = TrustGraphError("internal_error", repr(e))
        return ToolGuardrailFunctionOutput.reject_content(blocked_message(err, spec.name))
    return ToolGuardrailFunctionOutput.allow()


async def _register(source: str, data: ToolOutputGuardrailData) -> ToolGuardrailFunctionOutput:
    try:
        s, run = await _session(data.agent, data.context)
        await asyncio.to_thread(s.read, source, data.output)
        if run is not None:
            run.save(getattr(data.context, "context", None))
    except TrustGraphError as e:
        return ToolGuardrailFunctionOutput.reject_content(not_registered_message(source, e), output_info=e.code)
    except Exception as e:
        err = TrustGraphError("internal_error", repr(e))
        return ToolGuardrailFunctionOutput.reject_content(not_registered_message(source, err))
    return ToolGuardrailFunctionOutput.allow()


def _action_guardrail(spec: Action) -> ToolInputGuardrail[Any]:
    async def trustgraph_action(data: ToolInputGuardrailData) -> ToolGuardrailFunctionOutput:
        return await _check(spec, data)

    return ToolInputGuardrail(trustgraph_action, name=f"trustgraph_action:{spec.name}")


def _source_guardrail(source: str) -> ToolOutputGuardrail[Any]:
    async def trustgraph_source(data: ToolOutputGuardrailData) -> ToolGuardrailFunctionOutput:
        return await _register(source, data)

    return ToolOutputGuardrail(trustgraph_source, name=f"trustgraph_source:{source}")


def as_source(tool: FunctionTool, source: str) -> FunctionTool:
    """A copy of ``tool`` whose result is registered as ``source`` before the model sees it."""
    return dataclasses.replace(
        tool, tool_output_guardrails=[*(tool.tool_output_guardrails or []), _source_guardrail(source)]
    )


def as_action(tool: FunctionTool, action: str | Action) -> FunctionTool:
    """A copy of ``tool`` that is checked as ``action`` before it runs."""
    return dataclasses.replace(
        tool, tool_input_guardrails=[*(tool.tool_input_guardrails or []), _action_guardrail(as_spec(action))]
    )


_MCP = "trustgraph_mcp"


class _Hooks(RunHooks[Any]):
    def __init__(self, inner: RunHooks[Any] | None) -> None:
        self._inner = inner

    async def on_handoff(self, context: Any, from_agent: Agent[Any], to_agent: Agent[Any]) -> None:
        run = _run.get()
        if run is not None:
            # What the previous agent saw now travels with the work. If the
            # receiving agent is not cleared for it, this raises and the run stops.
            user = getattr(context, "context", None)
            sender = await asyncio.to_thread(run.session, from_agent.name, user)
            receiver = await asyncio.to_thread(run.session, to_agent.name, user)
            await asyncio.to_thread(receiver.receive, sender.handoff())
            run.save(user)
        if self._inner is not None:
            await self._inner.on_handoff(context, from_agent, to_agent)


def _forward(name: str) -> Any:
    async def method(self: _Hooks, *args: Any, **kwargs: Any) -> None:
        if self._inner is not None:
            await getattr(self._inner, name)(*args, **kwargs)

    return method


for _name, _ in inspect.getmembers(RunHooks, inspect.iscoroutinefunction):
    if _name.startswith("on_") and _name != "on_handoff":
        setattr(_Hooks, _name, _forward(_name))


class Guard:
    """Map tool names to sources and actions, and agent names to clients.

    ``protect(*agents)`` attaches the guardrails to those agents' function
    tools in place, and to the tools of their ``mcp_servers`` (matched by the
    name the model calls). ``run`` / ``run_sync`` / ``run_streamed`` call
    ``Runner`` with one session per agent for that run.
    """

    def __init__(
        self,
        clients: Mapping[str, Client] | None = None,
        sources: Mapping[str, str] | None = None,
        actions: Mapping[str, str | Action] | None = None,
    ) -> None:
        self.clients = dict(clients or {})
        self.sources = dict(sources or {})
        self.actions = {name: as_spec(a) for name, a in (actions or {}).items()}

    def protect(self, *agents: Agent[Any]) -> None:
        for agent in agents:
            tools = []
            for tool in agent.tools:
                if isinstance(tool, FunctionTool) and tool.name in self.actions:
                    tool = as_action(tool, self.actions[tool.name])
                if isinstance(tool, FunctionTool) and tool.name in self.sources:
                    tool = as_source(tool, self.sources[tool.name])
                tools.append(tool)
            agent.tools = tools
            for server in agent.mcp_servers:
                self._protect_server(server)

    def _protect_server(self, server: Any) -> None:
        """Every tool of an MCP server gets the server's guardrails (the SDK's hook for them)."""
        guard = self

        async def mcp_action(data: ToolInputGuardrailData) -> ToolGuardrailFunctionOutput:
            spec = guard.actions.get(data.context.tool_name)
            return ToolGuardrailFunctionOutput.allow() if spec is None else await _check(spec, data)

        async def mcp_source(data: ToolOutputGuardrailData) -> ToolGuardrailFunctionOutput:
            source = guard.sources.get(data.context.tool_name)
            return ToolGuardrailFunctionOutput.allow() if source is None else await _register(source, data)

        name = f"{_MCP}:{id(self)}"
        inputs = [g for g in (server.tool_input_guardrails or []) if g.name != name]
        outputs = [g for g in (server.tool_output_guardrails or []) if g.name != name]
        server.tool_input_guardrails = [*inputs, ToolInputGuardrail(mcp_action, name=name)]
        server.tool_output_guardrails = [*outputs, ToolOutputGuardrail(mcp_source, name=name)]

    def hooks(self, inner: RunHooks[Any] | None = None) -> RunHooks[Any]:
        """Run hooks that carry sessions across hand-offs; ``inner`` gets every event too."""
        return _Hooks(inner)

    @contextlib.contextmanager
    def scope(self, input: Any = None, *, _resumed_memory: bool | None = None, **run_kwargs: Any) -> Iterator[None]:
        """One run's sessions, for calling ``Runner`` yourself with ``hooks=guard.hooks()``.

        Pass the run's ``input`` and keyword arguments so a resumed conversation
        is recognised. A ``session=`` must be checked before the run starts, so
        use ``run`` / ``run_sync`` / ``run_streamed`` for that.
        """
        if run_kwargs.get("session") is not None and _resumed_memory is None:
            raise ValueError("with session=, use guard.run, guard.run_sync or guard.run_streamed")
        token = _run.set(_Run(self.clients, bool(_resumed_memory) or _resumed(input, run_kwargs)))
        try:
            yield
        finally:
            _run.reset(token)

    @staticmethod
    async def _has_history(kwargs: Mapping[str, Any]) -> bool:
        memory = kwargs.get("session")
        return memory is not None and bool(await memory.get_items(limit=1))

    async def run(self, starting_agent: Agent[Any], input: Any, **kwargs: Any) -> Any:
        """``Runner.run`` with fresh sessions for this run."""
        with self.scope(input, _resumed_memory=await self._has_history(kwargs), **kwargs):
            return await Runner.run(starting_agent, input, hooks=self.hooks(kwargs.pop("hooks", None)), **kwargs)

    def run_sync(self, starting_agent: Agent[Any], input: Any, **kwargs: Any) -> Any:
        """``Runner.run_sync`` with fresh sessions for this run."""
        history = asyncio.run(self._has_history(kwargs)) if kwargs.get("session") is not None else False
        with self.scope(input, _resumed_memory=history, **kwargs):
            return Runner.run_sync(starting_agent, input, hooks=self.hooks(kwargs.pop("hooks", None)), **kwargs)

    async def run_streamed(self, starting_agent: Agent[Any], input: Any, **kwargs: Any) -> Any:
        """``Runner.run_streamed`` with fresh sessions for this run: ``result = await guard.run_streamed(...)``."""
        with self.scope(input, _resumed_memory=await self._has_history(kwargs), **kwargs):
            # The run's task copies this context when it is created, so the scope follows it.
            return Runner.run_streamed(starting_agent, input, hooks=self.hooks(kwargs.pop("hooks", None)), **kwargs)

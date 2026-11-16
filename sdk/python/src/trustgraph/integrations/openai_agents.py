"""OpenAI Agents SDK.

Sources and actions are function-tool guardrails: an action is checked by a
tool *input* guardrail before the tool runs, and a source is registered by a
tool *output* guardrail before the model sees the result. A blocked call is
``reject_content``: the tool does not run (or its result is withheld) and the
model receives the reason.

``Guard.run`` gives each agent of a run its own session, made from that
agent's ``Client``, and carries what one agent's session has seen to the next
on every hand-off.
"""

from __future__ import annotations

import asyncio
import contextlib
import contextvars
import dataclasses
import inspect
import json
import threading
from collections.abc import Iterator, Mapping
from typing import Any

from agents import (
    Agent,
    FunctionTool,
    RunHooks,
    Runner,
    ToolGuardrailFunctionOutput,
    ToolInputGuardrail,
    ToolInputGuardrailData,
    ToolOutputGuardrail,
    ToolOutputGuardrailData,
)

from .._client import Client
from .._errors import TrustGraphError
from .._session import Session, current_session
from ._common import Action, as_spec, blocked_message, check_action, not_registered_message

__all__ = ["Action", "Guard", "as_action", "as_source"]


class _Run:
    """The sessions of one run, one per agent name."""

    def __init__(self, clients: Mapping[str, Client]) -> None:
        self.clients = dict(clients)
        self.sessions: dict[str, Session] = {}
        self.lock = threading.Lock()

    def session(self, agent: str) -> Session:
        with self.lock:
            if agent not in self.sessions:
                if agent not in self.clients:
                    raise TrustGraphError("no_session", f"no TrustGraph client for agent `{agent}`")
                self.sessions[agent] = self.clients[agent].session()
            return self.sessions[agent]


_run: contextvars.ContextVar[_Run | None] = contextvars.ContextVar("trustgraph_openai_run", default=None)


def _session(agent: Agent[Any]) -> Session:
    run = _run.get()
    if run is not None:
        return run.session(agent.name)
    s = current_session()
    if s is None:
        raise TrustGraphError("no_session", "no active TrustGraph session for this run")
    return s


def _args(data: ToolInputGuardrailData) -> dict[str, Any]:
    raw = getattr(data.context, "tool_arguments", None) or "{}"
    try:
        args = json.loads(raw)
    except ValueError:
        return {"arguments": raw}
    return args if isinstance(args, dict) else {"arguments": args}


def _action_guardrail(spec: Action) -> ToolInputGuardrail[Any]:
    async def trustgraph_action(data: ToolInputGuardrailData) -> ToolGuardrailFunctionOutput:
        try:
            s = _session(data.agent)
            await asyncio.to_thread(check_action, s, spec, _args(data))
        except TrustGraphError as e:
            return ToolGuardrailFunctionOutput.reject_content(blocked_message(e, spec.name), output_info=e.code)
        except Exception as e:  # fail closed on anything unexpected
            err = TrustGraphError("internal_error", repr(e))
            return ToolGuardrailFunctionOutput.reject_content(blocked_message(err, spec.name))
        return ToolGuardrailFunctionOutput.allow()

    return ToolInputGuardrail(trustgraph_action, name=f"trustgraph_action:{spec.name}")


def _source_guardrail(source: str) -> ToolOutputGuardrail[Any]:
    async def trustgraph_source(data: ToolOutputGuardrailData) -> ToolGuardrailFunctionOutput:
        try:
            s = _session(data.agent)
            await asyncio.to_thread(s.read, source, data.output)
        except TrustGraphError as e:
            return ToolGuardrailFunctionOutput.reject_content(not_registered_message(source, e), output_info=e.code)
        except Exception as e:
            err = TrustGraphError("internal_error", repr(e))
            return ToolGuardrailFunctionOutput.reject_content(not_registered_message(source, err))
        return ToolGuardrailFunctionOutput.allow()

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


class _Hooks(RunHooks[Any]):
    def __init__(self, inner: RunHooks[Any] | None) -> None:
        self._inner = inner

    async def on_handoff(self, context: Any, from_agent: Agent[Any], to_agent: Agent[Any]) -> None:
        run = _run.get()
        if run is not None:
            # What the previous agent saw now travels with the work. If the
            # receiving agent is not cleared for it, this raises and the run stops.
            token = run.session(from_agent.name).handoff()
            await asyncio.to_thread(run.session(to_agent.name).receive, token)
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
    tools in place. ``run`` / ``run_sync`` call ``Runner`` with one session per
    agent for that run.
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

    def hooks(self, inner: RunHooks[Any] | None = None) -> RunHooks[Any]:
        """Run hooks that carry sessions across hand-offs; ``inner`` gets every event too."""
        return _Hooks(inner)

    @contextlib.contextmanager
    def scope(self) -> Iterator[None]:
        """One run's sessions, for ``Runner.run_streamed`` with ``hooks=guard.hooks()``."""
        token = _run.set(_Run(self.clients))
        try:
            yield
        finally:
            _run.reset(token)

    async def run(self, starting_agent: Agent[Any], input: Any, **kwargs: Any) -> Any:
        """``Runner.run`` with fresh sessions for this run."""
        with self.scope():
            return await Runner.run(starting_agent, input, hooks=self.hooks(kwargs.pop("hooks", None)), **kwargs)

    def run_sync(self, starting_agent: Agent[Any], input: Any, **kwargs: Any) -> Any:
        """``Runner.run_sync`` with fresh sessions for this run."""
        with self.scope():
            return Runner.run_sync(starting_agent, input, hooks=self.hooks(kwargs.pop("hooks", None)), **kwargs)

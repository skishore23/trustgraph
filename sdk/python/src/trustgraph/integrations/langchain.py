"""LangChain and LangGraph.

- ``as_source(tool, "salaries")`` / ``as_action(tool, "post_to_engineering")``
  wrap one ``BaseTool`` (including ``@tool`` functions).
- ``Guard(sources=..., actions=...)`` maps tool names for a LangGraph
  ``ToolNode`` (``wrap_tool_call=``) or a LangChain ``create_agent``
  (``middleware=[guard.middleware()]``).
- ``agent_node(client, agent)`` runs a sub-agent in its own session and carries
  the hand-off token in graph state.

A blocked tool call returns a ``ToolMessage`` with ``status="error"`` and the
reason, so the model can explain it. The tool does not run.
"""

from __future__ import annotations

import asyncio
import contextvars
import functools
import threading
from collections.abc import Awaitable, Callable, Mapping
from typing import Any

from langchain_core.messages import ToolMessage
from langchain_core.runnables import RunnableConfig, RunnableLambda
from langchain_core.tools import BaseTool, ToolException

from .._client import Client
from .._errors import TrustGraphError
from .._session import Session, current_session
from ._common import Action, as_spec, blocked_message, check_action, not_registered_message, require_session

__all__ = ["Action", "Guard", "agent_node", "as_action", "as_source"]

_PLUMBING = {"run_manager", "callbacks", "config"}
# Set while a wrapped tool's async path runs, so a fallback to `_run` is not checked twice.
_inside: contextvars.ContextVar[bool] = contextvars.ContextVar("trustgraph_inside", default=False)


def _model_args(tool: BaseTool, kwargs: Mapping[str, Any]) -> dict[str, Any]:
    """The arguments the model supplied (not injected state or callbacks)."""
    try:
        fields = set(tool.tool_call_schema.model_fields)  # type: ignore[union-attr]
    except AttributeError:
        fields = set(kwargs) - _PLUMBING
    return {k: v for k, v in kwargs.items() if k in fields}


def _rewrap(tool: BaseTool, before: Callable[[BaseTool, dict[str, Any]], None] | None,
            after: Callable[[Any], None] | None) -> BaseTool:
    base = type(tool)

    # BaseTool.run reads `_run`'s signature to decide what to pass; keep it.
    @functools.wraps(base._run)
    def run(self: BaseTool, *args: Any, **kwargs: Any) -> Any:
        if _inside.get():
            return base._run(self, *args, **kwargs)
        if before:
            before(self, kwargs)
        result = base._run(self, *args, **kwargs)
        if after:
            after(result)
        return result

    @functools.wraps(base._arun)
    async def arun(self: BaseTool, *args: Any, **kwargs: Any) -> Any:
        if before:
            await asyncio.to_thread(before, self, kwargs)
        flag = _inside.set(True)
        try:
            result = await base._arun(self, *args, **kwargs)
        finally:
            _inside.reset(flag)
        if after:
            await asyncio.to_thread(after, result)
        return result

    wrapped_class = type(base.__name__, (base,), {"_run": run, "_arun": arun, "__module__": base.__module__})
    copy = tool.model_copy()
    object.__setattr__(copy, "__class__", wrapped_class)
    copy.handle_tool_error = True  # ToolException -> ToolMessage(status="error")
    return copy


def as_source(tool: BaseTool, source: str, *, content: Callable[[Any], Any] | None = None) -> BaseTool:
    """A copy of ``tool`` whose result is registered as ``source`` in the current session.

    If there is no session or registration fails, the result is withheld from
    the agent.
    """

    def before(_: BaseTool, __: dict[str, Any]) -> None:
        try:
            require_session()
        except TrustGraphError as e:
            raise ToolException(not_registered_message(source, e)) from None

    def after(result: Any) -> None:
        try:
            require_session().read(source, result if content is None else content(result))
        except TrustGraphError as e:
            raise ToolException(not_registered_message(source, e)) from None

    return _rewrap(tool, before, after)


def as_action(tool: BaseTool, action: str | Action) -> BaseTool:
    """A copy of ``tool`` that is checked as ``action`` before it runs."""
    spec = as_spec(action)

    def before(self: BaseTool, kwargs: dict[str, Any]) -> None:
        try:
            check_action(require_session(), spec, _model_args(self, kwargs))
        except TrustGraphError as e:
            raise ToolException(blocked_message(e, spec.name)) from None

    return _rewrap(tool, before, None)


class Guard:
    """Map tool names to sources and actions at the tool-execution step.

    The session is the current one (``with client.session():`` around the
    run). If there is none and ``client`` is given, each LangGraph thread
    (``config["configurable"]["thread_id"]``) gets its own session for the
    life of this guard. Tools in neither map run untouched.
    """

    def __init__(
        self,
        sources: Mapping[str, str] | None = None,
        actions: Mapping[str, str | Action] | None = None,
        *,
        client: Client | None = None,
    ) -> None:
        self.sources = dict(sources or {})
        self.actions = {name: as_spec(a) for name, a in (actions or {}).items()}
        self.client = client
        self._threads: dict[str, Session] = {}
        self._lock = threading.Lock()

    def session_for(self, config: Mapping[str, Any] | None) -> Session:
        s = current_session()
        if s is not None:
            return s
        thread = ((config or {}).get("configurable") or {}).get("thread_id")
        if self.client is None or thread is None:
            raise TrustGraphError("no_session", "no active TrustGraph session for this run")
        with self._lock:
            return self._threads.setdefault(str(thread), self.client.session())

    def _config(self, request: Any) -> Mapping[str, Any] | None:
        runtime = getattr(request, "runtime", None)
        return getattr(runtime, "config", None)

    @staticmethod
    def _error(request: Any, text: str) -> ToolMessage:
        call = request.tool_call
        return ToolMessage(content=text, tool_call_id=call["id"], name=call["name"], status="error")

    def _before(self, request: Any) -> ToolMessage | None:
        name = request.tool_call["name"]
        try:
            if name in self.actions:
                spec = self.actions[name]
                check_action(self.session_for(self._config(request)), spec, request.tool_call.get("args") or {})
            elif name in self.sources:
                self.session_for(self._config(request))
        except TrustGraphError as e:
            if name in self.actions:
                return self._error(request, blocked_message(e, self.actions[name].name))
            return self._error(request, not_registered_message(self.sources[name], e))
        return None

    def _after(self, request: Any, result: Any) -> Any:
        name = request.tool_call["name"]
        if name not in self.sources:
            return result
        source = self.sources[name]
        try:
            if not isinstance(result, ToolMessage):
                raise TrustGraphError("unsupported_result", "only a ToolMessage result can be registered")
            self.session_for(self._config(request)).read(source, result.content)
        except TrustGraphError as e:
            return self._error(request, not_registered_message(source, e))
        return result

    def wrap_tool_call(self, request: Any, execute: Callable[[Any], Any]) -> Any:
        """For ``ToolNode(wrap_tool_call=...)``."""
        blocked = self._before(request)
        if blocked is not None:
            return blocked
        return self._after(request, execute(request))

    async def awrap_tool_call(self, request: Any, execute: Callable[[Any], Awaitable[Any]]) -> Any:
        """For ``ToolNode(awrap_tool_call=...)``."""
        blocked = await asyncio.to_thread(self._before, request)
        if blocked is not None:
            return blocked
        result = await execute(request)
        return await asyncio.to_thread(self._after, request, result)

    def middleware(self) -> Any:
        """A LangChain ``AgentMiddleware`` for ``create_agent(middleware=[...])``."""
        from langchain.agents.middleware import AgentMiddleware

        guard = self

        class TrustGraphMiddleware(AgentMiddleware):  # type: ignore[misc]
            def wrap_tool_call(self, request: Any, handler: Callable[[Any], Any]) -> Any:
                return guard.wrap_tool_call(request, handler)

            async def awrap_tool_call(self, request: Any, handler: Callable[[Any], Awaitable[Any]]) -> Any:
                return await guard.awrap_tool_call(request, handler)

        return TrustGraphMiddleware()


def agent_node(client: Client, agent: Any, *, key: str = "trustgraph") -> RunnableLambda:
    """A graph node that runs ``agent`` (any runnable taking the graph state) in
    a new session of ``client``'s agent.

    The session starts carrying ``state[key]`` (the previous agent's hand-off)
    and the node writes its own hand-off token back to ``state[key]``. Declare
    ``key`` as a ``str`` field of the graph state. If the receiving agent is
    not cleared for what the token carries, the node raises and the graph run
    stops.
    """

    def update(state: Mapping[str, Any], result: Any, s: Session) -> Any:
        out = dict(result) if isinstance(result, Mapping) else {}
        out[key] = s.handoff()
        return out

    def run(state: Mapping[str, Any], config: RunnableConfig) -> Any:
        with client.session(carry=state.get(key)) as s:
            return update(state, agent.invoke(state, config), s)

    async def arun(state: Mapping[str, Any], config: RunnableConfig) -> Any:
        carry = state.get(key)
        s = await asyncio.to_thread(client.session, carry)
        async with s:
            return update(state, await agent.ainvoke(state, config), s)

    return RunnableLambda(run, afunc=arun, name=getattr(agent, "name", None) or "trustgraph_agent")

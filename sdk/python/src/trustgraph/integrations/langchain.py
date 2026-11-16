"""LangChain and LangGraph.

- ``as_source(tool, "salaries")`` / ``as_action(tool, "post_to_engineering")``
  wrap one ``BaseTool`` (including ``@tool`` functions).
- ``Guard(sources=..., actions=...)`` maps tool names for a LangGraph
  ``ToolNode`` (``wrap_tool_call=``) or a LangChain ``create_agent``
  (``middleware=[guard.middleware()]``).
- ``agent_node(client, agent, first=...)`` runs a sub-agent in its own session
  and carries the hand-off token in graph state.

A blocked tool call returns a ``ToolMessage`` with ``status="error"`` and the
reason, so the model can explain it. The tool does not run.
"""

from __future__ import annotations

import asyncio
import contextvars
import functools
import threading
import weakref
from collections import OrderedDict
from collections.abc import Awaitable, Callable, Mapping
from typing import Any

from langchain_core.messages import ToolMessage
from langchain_core.runnables import RunnableConfig, RunnableLambda
from langchain_core.tools import BaseTool, ToolException
from typing_extensions import NotRequired  # noqa: F401  (evaluated in the middleware's state schema)

from .._client import Client
from .._errors import TrustGraphError
from .._session import Session, current_session
from ._common import Action, as_spec, blocked_message, check_action, not_registered_message, require_session

__all__ = ["Action", "Guard", "agent_node", "as_action", "as_source"]

_PLUMBING = {"run_manager", "callbacks", "config"}
KEY = "trustgraph"  # the graph-state key that holds the hand-off token
# Set by a wrapped tool's async path to (tool, args, kwargs) while it awaits the
# tool's own `_arun`, so that `_arun`'s fallback to the same tool's `_run`, with
# the same arguments, is not checked twice. Anything else is checked.
_fallback: contextvars.ContextVar[tuple[BaseTool, tuple[Any, ...], dict[str, Any]] | None] = (
    contextvars.ContextVar("trustgraph_fallback", default=None)
)


def _model_args(tool: BaseTool, kwargs: Mapping[str, Any]) -> dict[str, Any]:
    """The arguments the model supplied (not injected state or callbacks)."""
    try:
        fields = set(tool.tool_call_schema.model_fields)  # type: ignore[union-attr]
    except AttributeError:
        fields = set(kwargs) - _PLUMBING
    return {k: v for k, v in kwargs.items() if k in fields}


def _is_own_fallback(self: BaseTool, args: tuple[Any, ...], kwargs: dict[str, Any]) -> bool:
    marker = _fallback.get()
    if marker is None:
        return False
    tool, margs, mkwargs = marker
    # BaseTool._arun passes the very same argument objects on (run_manager becomes its sync twin).
    return (
        tool is self
        and len(args) == len(margs)
        and all(a is b for a, b in zip(args, margs))
        and set(kwargs) == set(mkwargs)
        and all(kwargs[k] is mkwargs[k] for k in kwargs if k != "run_manager")
    )


def _rewrap(tool: BaseTool, before: Callable[[BaseTool, dict[str, Any]], None] | None,
            after: Callable[[Any], None] | None) -> BaseTool:
    base = type(tool)

    # BaseTool.run reads `_run`'s signature to decide what to pass; keep it.
    @functools.wraps(base._run)
    def run(self: BaseTool, *args: Any, **kwargs: Any) -> Any:
        if _is_own_fallback(self, args, kwargs):
            # Checked by `arun` already. Clear the marker first, so a wrapped tool
            # the body calls is checked. (This context is a copy: `arun`'s is untouched.)
            _fallback.set(None)
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
        marker = _fallback.set((self, args, dict(kwargs)))
        try:
            result = await base._arun(self, *args, **kwargs)
        finally:
            _fallback.reset(marker)
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


def _messages(state: Any) -> list[Any]:
    if isinstance(state, Mapping):
        state = state.get("messages")
    elif not isinstance(state, list):
        state = getattr(state, "messages", None)
    return list(state) if isinstance(state, list) else []


class Guard:
    """Map tool names to sources and actions at the tool-execution step.

    The session is the current one (``with client.session():`` around the
    run). If there is none and ``client`` is given, each LangGraph thread
    (``config["configurable"]["thread_id"]``) gets its own session, restored
    from the hand-off token in ``state["trustgraph"]`` when there is one; the
    guard writes that token back after every registered result. At most
    ``max_threads`` thread sessions are kept in memory (least recently used
    first out). Tools in neither map run untouched.

    A mapped call is refused when the conversation already holds a result of a
    mapped source tool that the session did not record and the session carries
    no hand-off token: that is a resumed, checkpointed or evicted thread with a
    new session, which would drop the restriction.
    """

    def __init__(
        self,
        sources: Mapping[str, str] | None = None,
        actions: Mapping[str, str | Action] | None = None,
        *,
        client: Client | None = None,
        max_threads: int = 1024,
    ) -> None:
        if max_threads < 1:
            raise ValueError("max_threads must be at least 1")
        self.sources = dict(sources or {})
        self.actions = {name: as_spec(a) for name, a in (actions or {}).items()}
        self.client = client
        self.max_threads = max_threads
        self._threads: OrderedDict[str, Session] = OrderedDict()
        self._recorded: weakref.WeakKeyDictionary[Session, set[str]] = weakref.WeakKeyDictionary()
        self._lock = threading.Lock()

    def session_for(self, config: Mapping[str, Any] | None, state: Any = None) -> Session:
        """The session for a call: the current one, else the thread's (``client=`` only)."""
        s = current_session()
        if s is not None:
            return s
        return self._thread_session(config, state)

    def _thread_session(self, config: Mapping[str, Any] | None, state: Any) -> Session:
        thread = ((config or {}).get("configurable") or {}).get("thread_id")
        if self.client is None or thread is None:
            raise TrustGraphError("no_session", "no active TrustGraph session for this run")
        thread = str(thread)
        with self._lock:
            s = self._threads.get(thread)
            if s is not None:
                self._threads.move_to_end(thread)
                return s
        token = state.get(KEY) if isinstance(state, Mapping) else None
        s = self.client.session()
        if token:
            try:
                s.receive(token)  # a failure breaks `s`: every mapped call of the thread is refused
            except TrustGraphError:
                pass
        with self._lock:
            s = self._threads.setdefault(thread, s)
            self._threads.move_to_end(thread)
            while len(self._threads) > self.max_threads:
                self._threads.popitem(last=False)
        return s

    def _owned(self) -> bool:
        return current_session() is None

    def _check_history(self, s: Session, request: Any) -> None:
        if s.carried:
            return
        recorded = self._recorded.get(s, set())
        # `request.state` is the state before this call, so it never holds this call's own result.
        for m in _messages(getattr(request, "state", None)):
            if (
                isinstance(m, ToolMessage)
                and m.name in self.sources
                and m.status != "error"
                and m.tool_call_id not in recorded
            ):
                raise TrustGraphError(
                    "unrecorded_history",
                    f"this conversation already holds a result of `{m.name}` that this TrustGraph session did "
                    "not record (a resumed or checkpointed thread with a new session); carry the thread's "
                    f"hand-off token in `state[{KEY!r}]`, or start a new thread",
                )

    def _config(self, request: Any) -> Mapping[str, Any] | None:
        runtime = getattr(request, "runtime", None)
        return getattr(runtime, "config", None)

    def _session(self, request: Any) -> Session:
        s = self.session_for(self._config(request), getattr(request, "state", None))
        self._check_history(s, request)
        return s

    @staticmethod
    def _error(request: Any, text: str) -> ToolMessage:
        call = request.tool_call
        return ToolMessage(content=text, tool_call_id=call["id"], name=call["name"], status="error")

    def _before(self, request: Any) -> ToolMessage | None:
        name = request.tool_call["name"]
        try:
            if name in self.actions:
                spec = self.actions[name]
                check_action(self._session(request), spec, request.tool_call.get("args") or {})
            elif name in self.sources:
                self._session(request)
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
            owned = self._owned()
            s = self._session(request)
            s.read(source, result.content)
            with self._lock:
                self._recorded.setdefault(s, set()).add(result.tool_call_id)
            if owned:
                from langgraph.types import Command

                # Keep the thread's token with the thread's checkpoint, to restore from.
                return Command(update={"messages": [result], KEY: s.handoff()})
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
        """A LangChain ``AgentMiddleware`` for ``create_agent(middleware=[...])``.

        Put it last in the list (innermost), so it checks the arguments the
        tool actually runs with."""
        from langchain.agents.middleware import AgentMiddleware, AgentState

        guard = self

        class TrustGraphState(AgentState):  # type: ignore[misc]
            trustgraph: NotRequired[str]

        class TrustGraphMiddleware(AgentMiddleware):  # type: ignore[misc]
            state_schema = TrustGraphState

            def wrap_tool_call(self, request: Any, handler: Callable[[Any], Any]) -> Any:
                return guard.wrap_tool_call(request, handler)

            async def awrap_tool_call(self, request: Any, handler: Callable[[Any], Awaitable[Any]]) -> Any:
                return await guard.awrap_tool_call(request, handler)

        return TrustGraphMiddleware()


def _has_history(state: Mapping[str, Any]) -> bool:
    return any(getattr(m, "type", None) in ("ai", "tool") for m in _messages(state))


def agent_node(client: Client, agent: Any, *, key: str = KEY, first: bool = False) -> RunnableLambda:
    """A graph node that runs ``agent`` (any runnable taking the graph state) in
    a new session of ``client``'s agent.

    The session starts carrying ``state[key]`` (the previous agent's hand-off)
    and the node writes its own hand-off token back to ``state[key]``. Declare
    ``key: str`` in the graph's state schema and start the graph with
    ``{key: ""}``: LangGraph silently drops undeclared keys, so the node
    raises ``handoff_missing`` before running the agent when the key is absent.
    Only the node marked ``first=True`` may start from an empty token, and not
    when the state already holds model or tool messages (a resumed thread whose
    token was reset). Every other node requires a valid token. If the receiving
    agent is not cleared for what the token carries, the node raises and the
    graph run stops.
    """

    def carry(state: Any) -> str | None:
        if not isinstance(state, Mapping) or key not in state:
            raise TrustGraphError(
                "handoff_missing",
                f"the graph state has no `{key}` key, so the hand-off would be lost: declare `{key}: str` in the "
                f"state schema and start the graph with {{{key!r}: ''}}",
            )
        token = state[key]
        if token:
            return str(token)
        if not first:
            raise TrustGraphError(
                "handoff_missing",
                f"`{key}` is empty, so this agent would start without what the agents before it read; only the "
                "node made with agent_node(..., first=True) may start without a hand-off",
            )
        if _has_history(state):
            raise TrustGraphError(
                "resumed_without_session",
                f"`{key}` is empty but the conversation already holds model or tool messages (a resumed thread); "
                f"keep the thread's `{key}` from its checkpoint instead of passing an empty one",
            )
        return None

    def update(state: Mapping[str, Any], result: Any, s: Session) -> Any:
        out = dict(result) if isinstance(result, Mapping) else {}
        out[key] = s.handoff()
        return out

    def run(state: Mapping[str, Any], config: RunnableConfig) -> Any:
        with client.session(carry=carry(state)) as s:
            return update(state, agent.invoke(state, config), s)

    async def arun(state: Mapping[str, Any], config: RunnableConfig) -> Any:
        token = carry(state)
        s = await asyncio.to_thread(client.session, token)
        async with s:
            return update(state, await agent.ainvoke(state, config), s)

    return RunnableLambda(run, afunc=arun, name=getattr(agent, "name", None) or "trustgraph_agent")

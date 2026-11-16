"""``@source`` and ``@action`` for plain functions, sync or async."""

from __future__ import annotations

import asyncio
import functools
import inspect
import time
from collections.abc import Callable, Iterable
from typing import Any, TypeVar

from ._errors import Denied, NeedsApproval, TrustGraphError
from ._session import Session, current_session
from ._types import Decision

F = TypeVar("F", bound=Callable[..., Any])

ContentFn = Callable[[tuple[Any, ...], dict[str, Any]], Any]
Recipients = Iterable[str] | Callable[[tuple[Any, ...], dict[str, Any]], "Iterable[str] | None"] | None


def _require() -> Session:
    s = current_session()
    if s is None:
        raise TrustGraphError(
            "no_session", "no active TrustGraph session; wrap the agent's run in `with client.session():`"
        )
    return s


def source(name: str, *, content: Callable[[Any], Any] | None = None, require_session: bool = True) -> Callable[[F], F]:
    """Register the function's return value as source ``name`` in the current session.

    ``content`` picks what to hash from the return value (default: the value
    itself, which must be text, bytes or JSON; anything else raises
    ``InvalidContent`` and the value is not returned). A generator function
    needs ``content``. With ``require_session=False`` and no active session the
    function runs untracked.
    """

    def pick(result: Any) -> Any:
        return result if content is None else content(result)

    def decorate(fn: F) -> F:
        if content is None and (inspect.isgeneratorfunction(fn) or inspect.isasyncgenfunction(fn)):
            raise TypeError(
                f"@source({name!r}) cannot register what a generator yields; "
                "pass `content=` to pick what to hash, or return the value"
            )
        if inspect.iscoroutinefunction(fn):

            @functools.wraps(fn)
            async def async_wrapper(*args: Any, **kwargs: Any) -> Any:
                s = current_session()
                if s is None and require_session:
                    _require()
                result = await fn(*args, **kwargs)
                if s is not None:
                    await asyncio.to_thread(s.read, name, pick(result))
                return result

            return async_wrapper  # type: ignore[return-value]

        @functools.wraps(fn)
        def wrapper(*args: Any, **kwargs: Any) -> Any:
            s = current_session()
            if s is None and require_session:
                _require()
            result = fn(*args, **kwargs)
            if s is not None:
                s.read(name, pick(result))
            return result

        return wrapper  # type: ignore[return-value]

    return decorate


def _default_content(args: tuple[Any, ...], kwargs: dict[str, Any]) -> Any:
    return {"args": list(args), "kwargs": kwargs}


def action(
    name: str,
    *,
    content: ContentFn | None = None,
    recipients: Recipients = None,
    step: str | None = None,
) -> Callable[[F], F]:
    """Check action ``name`` before the function body runs; run it only on allow.

    ``content(args, kwargs)`` picks what is recorded as the output (default:
    all arguments). ``recipients`` is a list, or a function of ``(args,
    kwargs)``; a ``str`` is one name, and ``None`` means the action's whole
    audience. Raises ``Denied``,
    ``NeedsApproval`` or ``TrustGraphError`` instead of running. There is no
    way to run it without a session.
    """
    pick = content or _default_content

    def check(args: tuple[Any, ...], kwargs: dict[str, Any]) -> Decision:
        s = _require()
        who = recipients(args, kwargs) if callable(recipients) else recipients
        return s.act(name, pick(args, kwargs), who, step=step)

    def decorate(fn: F) -> F:
        if inspect.iscoroutinefunction(fn):

            @functools.wraps(fn)
            async def async_wrapper(*args: Any, **kwargs: Any) -> Any:
                _require()
                await asyncio.to_thread(check, args, kwargs)
                return await fn(*args, **kwargs)

            return async_wrapper  # type: ignore[return-value]

        @functools.wraps(fn)
        def wrapper(*args: Any, **kwargs: Any) -> Any:
            check(args, kwargs)
            return fn(*args, **kwargs)

        return wrapper  # type: ignore[return-value]

    return decorate


def wait_for_approval(decision: Decision, timeout: float = 300.0, poll: float = 2.0) -> Decision:
    """Re-check a needs-approval decision until it allows.

    Returns the allowing decision; raises ``Denied`` if it turns to deny and
    ``NeedsApproval`` if ``timeout`` seconds pass first. Run the action again
    after it returns: the action's own check then allows.
    """
    client = decision._client
    if client is None:
        raise TrustGraphError("no_client", "this decision was not made by a Client")
    deadline = time.monotonic() + timeout
    while True:
        if decision.verdict == "allow":
            return decision
        if decision.verdict == "deny":
            raise Denied(decision)
        if time.monotonic() >= deadline:
            raise NeedsApproval(decision)
        time.sleep(max(0.0, min(poll, deadline - time.monotonic())))
        decision = client.check(decision.artifact, decision.action, decision.recipients)

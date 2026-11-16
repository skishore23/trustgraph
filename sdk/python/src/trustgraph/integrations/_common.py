"""What every integration shares: action specs and the message an agent sees when blocked."""

from __future__ import annotations

from collections.abc import Callable, Iterable, Mapping
from dataclasses import dataclass
from typing import Any

from .._errors import Denied, NeedsApproval, TrustGraphError
from .._session import Session, current_session

ArgsFn = Callable[[Mapping[str, Any]], Any]


@dataclass(frozen=True)
class Action:
    """A policy action for a tool. ``content(args)`` picks what is recorded as the
    output (default: all arguments); ``recipients(args)`` names the recipients
    (default: the action's whole audience)."""

    name: str
    content: ArgsFn | None = None
    recipients: Callable[[Mapping[str, Any]], Iterable[str] | None] | None = None


def as_spec(action: str | Action) -> Action:
    return action if isinstance(action, Action) else Action(action)


def check_action(session: Session, spec: Action, args: Mapping[str, Any]) -> None:
    """Record the call as an output of everything seen and check it; raise unless allowed."""
    content = dict(args) if spec.content is None else spec.content(args)
    recipients = None if spec.recipients is None else spec.recipients(args)
    session.act(spec.name, content, recipients)


def require_session() -> Session:
    s = current_session()
    if s is None:
        raise TrustGraphError("no_session", "no active TrustGraph session for this run")
    return s


def blocked_message(error: TrustGraphError, action: str | None = None) -> str:
    """The text returned to the agent in place of the tool result."""
    if isinstance(error, Denied):
        return f"TrustGraph blocked `{action}`: {error.decision.reason}. The tool did not run."
    if isinstance(error, NeedsApproval):
        return (
            f"TrustGraph held `{action}`: {error.decision.reason} "
            f"(decision {error.decision.id}). The tool did not run; try again after a reviewer approves."
        )
    what = f"`{action}`" if action else "this tool"
    return (
        f"TrustGraph could not complete the check for {what} ({error.code}: {error.message}). "
        "The tool did not run."
    )


def not_registered_message(source: str, error: TrustGraphError) -> str:
    return (
        f"TrustGraph could not register the result as source `{source}` ({error.code}: {error.message}). "
        "The result was withheld."
    )

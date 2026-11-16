"""Exceptions. Every one means the same thing to an action: it did not run."""

from __future__ import annotations

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from ._types import Decision


class TrustGraphError(Exception):
    """A check could not be completed, or the server refused a request.

    ``status`` is the HTTP status (``None`` when the server was not reached or
    the SDK refused before sending); ``code`` is the server's error code or one
    of the SDK's own (``unreachable``, ``invalid_response``, ``no_session``,
    ``nothing_read``, ``session_broken``, ``invalid_handoff``).
    """

    def __init__(self, code: str, message: str, status: int | None = None) -> None:
        super().__init__(f"{code}: {message}" if status is None else f"{status} {code}: {message}")
        self.code = code
        self.message = message
        self.status = status


class Denied(TrustGraphError):
    """The policy denied the action. ``decision`` says why and which sources block it."""

    def __init__(self, decision: Decision) -> None:
        super().__init__(decision.code, decision.reason)
        self.decision = decision


class NeedsApproval(TrustGraphError):
    """The action needs a reviewer's approval first (see ``wait_for_approval``)."""

    def __init__(self, decision: Decision) -> None:
        super().__init__(decision.code, decision.reason)
        self.decision = decision

    @property
    def decision_id(self) -> str:
        return self.decision.id

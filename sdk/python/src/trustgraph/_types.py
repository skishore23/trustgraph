"""Value types returned by the client."""

from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Any

from ._errors import Denied, InvalidContent, NeedsApproval, TrustGraphError

if TYPE_CHECKING:
    from ._client import Client

VERDICTS = ("allow", "needs_approval", "deny")


@dataclass(frozen=True)
class Artifact:
    """Something registered with the server: a source the agent read or an output it wrote."""

    id: str

    def __str__(self) -> str:
        return self.id


@dataclass(frozen=True)
class Blocking:
    """A source that keeps an output from reaching the action's recipients."""

    source: str
    readers: tuple[str, ...]
    excluded: tuple[str, ...]


@dataclass(frozen=True)
class Decision:
    """The answer to a check. Act only when ``allowed`` is true."""

    id: str
    verdict: str
    code: str
    reason: str
    blocking: tuple[Blocking, ...]
    expires_at: int | None
    artifact: str
    action: str
    recipients: tuple[str, ...] | None
    _client: Client | None = field(default=None, repr=False, compare=False)

    @property
    def allowed(self) -> bool:
        return self.verdict == "allow"

    def raise_unless_allowed(self) -> Decision:
        """Return the decision if it allows; raise ``Denied`` or ``NeedsApproval`` otherwise."""
        if self.verdict == "allow":
            return self
        if self.verdict == "needs_approval":
            raise NeedsApproval(self)
        raise Denied(self)

    @classmethod
    def _from_json(
        cls,
        body: Any,
        artifact: str,
        action: str,
        recipients: tuple[str, ...] | None,
        client: Client | None,
    ) -> Decision:
        try:
            verdict = body["verdict"]
            if verdict not in VERDICTS:
                raise ValueError(verdict)
            blocking = tuple(
                Blocking(b["source"], tuple(b.get("readers", ())), tuple(b.get("excluded", ())))
                for b in body.get("blocking") or ()
            )
            return cls(
                id=str(body["decision"]),
                verdict=verdict,
                code=str(body["code"]),
                reason=str(body["reason"]),
                blocking=blocking,
                expires_at=body.get("expires_at"),
                artifact=artifact,
                action=action,
                recipients=recipients,
                _client=client,
            )
        except (KeyError, TypeError, ValueError) as e:
            raise TrustGraphError("invalid_response", f"not a check answer: {e!r}") from None


def to_bytes(content: Any) -> bytes:
    """The bytes that stand for ``content``: text as UTF-8, bytes as they are,
    anything else as compact JSON with sorted keys.

    Raises ``InvalidContent`` for ``None`` and for values that are not JSON
    (generators, arbitrary objects): hashing their ``repr()`` would register
    something other than what was read.
    """
    if content is None:
        raise InvalidContent("the content is None; pass the value that was read or written")
    if isinstance(content, (bytes, bytearray)):
        return bytes(content)
    if isinstance(content, str):
        return content.encode()
    try:
        text = json.dumps(content, sort_keys=True, separators=(",", ":"), ensure_ascii=False, allow_nan=False)
    except (TypeError, ValueError) as e:
        raise InvalidContent(
            f"cannot hash a {type(content).__name__} value: it is not text, bytes or JSON ({e}); "
            "pass `content=` to pick what to hash"
        ) from None
    return text.encode()


def sha256_hex(content: Any) -> str:
    return hashlib.sha256(to_bytes(content)).hexdigest()

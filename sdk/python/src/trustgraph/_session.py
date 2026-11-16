"""Sessions: what an agent read follows what it writes."""

from __future__ import annotations

import base64
import binascii
import contextvars
import hashlib
import json
import re
import threading
from collections.abc import Iterable
from types import TracebackType
from typing import TYPE_CHECKING, Any, TypeVar

from ._errors import TrustGraphError
from ._types import Artifact, Decision

if TYPE_CHECKING:
    from ._client import Client

T = TypeVar("T")

_current: contextvars.ContextVar[Session | None] = contextvars.ContextVar("trustgraph_session", default=None)

_ID = re.compile(r"^[0-9a-f]{64}$")
_PREFIX = "tg1."
MAX_INPUTS = 64  # the server's limit per output
MERGE_STEP = "trustgraph.merge"


def current_session() -> Session | None:
    """The session active in this context, if any."""
    return _current.get()


class Session:
    """Everything read in a session attaches to everything written later.

    Use it as a context manager (``with client.session() as s:``) to make it
    the current session for decorators and integrations; its methods also work
    without that.
    """

    def __init__(self, client: Client, carry: str | None = None) -> None:
        self.client = client
        self._seen: list[str] = []
        self._lock = threading.Lock()
        self._broken: str | None = None
        self._tokens: list[contextvars.Token[Session | None]] = []
        if carry is not None:
            self.receive(carry)

    # -- context ---------------------------------------------------------

    def __enter__(self) -> Session:
        self._tokens.append(_current.set(self))
        return self

    def __exit__(
        self, exc_type: type[BaseException] | None, exc: BaseException | None, tb: TracebackType | None
    ) -> None:
        _current.reset(self._tokens.pop())

    async def __aenter__(self) -> Session:
        return self.__enter__()

    async def __aexit__(
        self, exc_type: type[BaseException] | None, exc: BaseException | None, tb: TracebackType | None
    ) -> None:
        self.__exit__(exc_type, exc, tb)

    @property
    def seen(self) -> tuple[str, ...]:
        """The artifact ids everything written in this session derives from."""
        with self._lock:
            return tuple(self._seen)

    def _add(self, artifact: Artifact) -> None:
        with self._lock:
            if artifact.id not in self._seen:
                self._seen.append(artifact.id)

    def _usable(self) -> None:
        if self._broken is not None:
            raise TrustGraphError("session_broken", self._broken)

    # -- reading and writing --------------------------------------------

    def read(self, source: str, content: T) -> T:
        """Register ``content`` as ``source`` and return it unchanged."""
        self._usable()
        self._add(self.client.register_source(source, content))
        return content

    def output(self, content: Any, step: str = "output") -> Artifact:
        """Record ``content`` as derived from everything seen so far."""
        self._usable()
        artifact = self.client.record_output(self._inputs(), step, content)
        self._add(artifact)
        return artifact

    def act(self, action: str, content: Any, recipients: Iterable[str] | None = None, step: str | None = None) -> Decision:
        """Record ``content`` as an output of everything seen and check ``action``.

        Returns the decision when it allows; raises ``Denied`` or
        ``NeedsApproval`` otherwise, and ``TrustGraphError`` when the check
        could not be completed.
        """
        self._usable()
        artifact = self.client.record_output(self._inputs(), step or action, content)
        return self.client.check(artifact, action, recipients).raise_unless_allowed()

    def _inputs(self) -> list[str]:
        seen = list(self.seen)
        if not seen:
            raise TrustGraphError(
                "nothing_read",
                "this session has not read anything, so there is nothing to check the output against; "
                "register the source it came from",
            )
        # The server takes at most 64 inputs per output. Fold larger sets into
        # merge outputs: an output's readers are those of every input, so a
        # merge carries exactly the restrictions of what it folds.
        while len(seen) > MAX_INPUTS:
            seen = [self._merge(seen[i : i + MAX_INPUTS]) for i in range(0, len(seen), MAX_INPUTS)]
        return seen

    def _merge(self, ids: list[str]) -> str:
        if len(ids) == 1:
            return ids[0]
        digest = hashlib.sha256("\n".join(ids).encode()).hexdigest()
        return self.client.record_output(ids, MERGE_STEP, sha256=digest).id

    # -- hand-off --------------------------------------------------------

    def handoff(self) -> str:
        """An opaque token carrying what this session has seen, for another agent's session."""
        self._usable()
        payload = json.dumps({"artifacts": list(self.seen)}, separators=(",", ":")).encode()
        return _PREFIX + base64.urlsafe_b64encode(payload).decode().rstrip("=")

    def receive(self, token: str) -> None:
        """Carry what another agent's session had seen.

        The receiving agent must be cleared (listed in ``agents``) for every
        source the token carries. If it is not, or the token is malformed, this
        raises and the session refuses all further writes and checks.
        """
        try:
            ids = _parse(token)
            for artifact in ids:
                self.client.explain(artifact)
        except TrustGraphError as e:
            self._broken = f"a hand-off could not be received ({e.code}); start a new session"
            raise
        for artifact in ids:
            self._add(Artifact(artifact))


def _parse(token: str) -> list[str]:
    try:
        if not isinstance(token, str) or not token.startswith(_PREFIX):
            raise ValueError("prefix")
        raw = token[len(_PREFIX) :]
        payload = json.loads(base64.urlsafe_b64decode(raw + "=" * (-len(raw) % 4)))
        ids = payload["artifacts"]
        if not isinstance(ids, list) or not all(isinstance(i, str) and _ID.match(i) for i in ids):
            raise ValueError("artifacts")
        return ids
    except (ValueError, KeyError, TypeError, binascii.Error):
        raise TrustGraphError("invalid_handoff", "not a TrustGraph hand-off token") from None


def session(carry: str | None = None) -> Session:
    """A new session on the default client."""
    from ._client import default_client

    return default_client().session(carry=carry)

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
        self.carried = False  # True once a hand-off token was received
        self._results: list[str] = []  # tool-call ids of source results recorded (by an integration)
        self._unlisted = 0  # results a carried token counted but no longer lists
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

    @property
    def broken(self) -> str | None:
        """Why this session refuses every read, output, check and hand-off, or ``None``."""
        return self._broken

    def mark_broken(self, reason: str) -> None:
        """Refuse every later read, output, check and hand-off of this session. It cannot be undone."""
        if self._broken is None:
            self._broken = reason

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

    # -- tool results ----------------------------------------------------

    def record_result(self, call_id: str) -> None:
        """Note that the source result of tool call ``call_id`` was handled by this
        session. Integrations use it to tell a result they saw from one they never did."""
        with self._lock:
            if call_id not in self._results:
                self._results.append(call_id)

    @property
    def results(self) -> tuple[str, ...]:
        """The tool-call ids of the source results this session (or what it carried) handled."""
        with self._lock:
            return tuple(self._results)

    @property
    def unlisted(self) -> int:
        """How many handled results a carried token counted beyond those it still lists."""
        return self._unlisted

    def _payload(self, broken: str | None) -> str:
        with self._lock:
            return _token(self._seen, broken, self._results, len(self._results) + self._unlisted)

    # -- hand-off --------------------------------------------------------

    def handoff(self) -> str:
        """An opaque token carrying what this session has seen, for another agent's session.

        Raises ``session_broken`` on a broken session: a hand-off is only ever
        written from a session that is not broken (see ``persist``)."""
        self._usable()
        return self._payload(None)

    def persist(self, stored: str | None = None) -> str:
        """The token to keep where this conversation is kept, merged with ``stored``.

        Unlike ``handoff`` it never raises: it is the union of ``stored`` and
        everything this session has seen, and if this session (or ``stored``)
        is broken it carries a broken marker, so a session restored from it is
        refused too. Saving it can never shed an artifact or a broken marker.
        """
        return merge_tokens(stored, self._payload(self._broken))

    def receive(self, token: str) -> None:
        """Carry what another agent's session had seen.

        The receiving agent must be cleared (listed in ``agents``) for every
        source the token carries. If it is not, the token is malformed, or it
        carries a broken marker, this raises and the session refuses all
        further reads, writes, checks and hand-offs.
        """
        try:
            ids, broken, results, total = _parse_full(token)
        except TrustGraphError as e:
            self.mark_broken(f"a hand-off could not be received ({e.code}); start a new session")
            raise
        for artifact in ids:  # kept even on failure, so that `persist` never sheds them
            self._add(Artifact(artifact))
        with self._lock:
            new = [r for r in results if r not in self._results]
            self._results.extend(new)
            self._unlisted = max(self._unlisted, total - len(results))
        try:
            if broken is not None:
                raise TrustGraphError("session_broken", f"the carried session was broken: {broken}")
            for artifact in ids:
                self.client.explain(artifact)
        except TrustGraphError as e:
            self.mark_broken(
                f"the carried session was broken: {broken}" if broken is not None
                else f"a hand-off could not be received ({e.code}); start a new session"
            )
            raise
        self.carried = True


MAX_RESULTS = 256  # tool-call ids a token lists; older ones are only counted


def _token(ids: Iterable[str], broken: str | None = None, results: Iterable[str] = (), total: int = 0) -> str:
    body: dict[str, Any] = {"artifacts": list(ids)}
    if broken is not None:
        body["broken"] = broken
    listed = list(results)[-MAX_RESULTS:]
    if listed:
        body["results"] = listed
    if total > len(listed):
        body["results_total"] = total
    payload = json.dumps(body, separators=(",", ":")).encode()
    return _PREFIX + base64.urlsafe_b64encode(payload).decode().rstrip("=")


def _parse_full(token: str) -> tuple[list[str], str | None, list[str], int]:
    try:
        if not isinstance(token, str) or not token.startswith(_PREFIX):
            raise ValueError("prefix")
        raw = token[len(_PREFIX) :]
        payload = json.loads(base64.urlsafe_b64decode(raw + "=" * (-len(raw) % 4)))
        ids = payload["artifacts"]
        if not isinstance(ids, list) or not all(isinstance(i, str) and _ID.match(i) for i in ids):
            raise ValueError("artifacts")
        broken = payload.get("broken")
        if broken is not None and not isinstance(broken, str):
            raise ValueError("broken")
        results = payload.get("results", [])
        if not isinstance(results, list) or not all(isinstance(r, str) for r in results):
            raise ValueError("results")
        total = payload.get("results_total", len(results))
        if not isinstance(total, int) or isinstance(total, bool) or total < len(results):
            raise ValueError("results_total")
        return ids, broken, results, total
    except (ValueError, KeyError, TypeError, AttributeError, binascii.Error):
        raise TrustGraphError("invalid_handoff", "not a TrustGraph hand-off token") from None


def _parse(token: str) -> tuple[list[str], str | None]:
    ids, broken, _, _ = _parse_full(token)
    return ids, broken


def merge_tokens(left: str | None, right: str | None, *more: str | None) -> str:
    """The union of tokens: every artifact and recorded result any of them
    carries, and a broken marker if any of them is broken or cannot be read.
    Empty values are ignored; with nothing to merge the result is ``""``.

    It never yields a token carrying fewer artifacts than an input, so it is
    safe as a reducer for state that keeps a token (LangGraph:
    ``Annotated[str, merge_tokens]``)."""
    ids: dict[str, None] = {}
    results: dict[str, None] = {}
    total = 0
    broken: str | None = None
    found = False
    for token in (left, right, *more):
        if not token:
            continue
        found = True
        try:
            artifacts, why, listed, counted = _parse_full(token)
        except TrustGraphError:
            artifacts, why, listed, counted = [], "an unreadable hand-off token was merged into this one", [], 0
        ids.update(dict.fromkeys(artifacts))
        results.update(dict.fromkeys(listed))
        total = max(total, counted)
        if broken is None:
            broken = why
    return _token(ids, broken, results, max(total, len(results))) if found else ""


def session(carry: str | None = None) -> Session:
    """A new session on the default client."""
    from ._client import default_client

    return default_client().session(carry=carry)

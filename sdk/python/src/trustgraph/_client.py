"""One agent's connection to ``trustgraph serve``."""

from __future__ import annotations

import json
import os
import re
import urllib.error
import urllib.request
from collections.abc import Callable, Iterable
from typing import TYPE_CHECKING, Any

from ._errors import TrustGraphError
from ._types import Artifact, Decision, sha256_hex

if TYPE_CHECKING:
    from ._session import Session

#: ``transport(method, url, headers, body) -> (status, body)``. It raises
#: ``OSError`` when the server cannot be reached.
Transport = Callable[[str, str, dict[str, str], "bytes | None", float], "tuple[int, bytes]"]

_ID = re.compile(r"^[0-9a-f]{64}$")


def _urllib_transport(
    method: str, url: str, headers: dict[str, str], body: bytes | None, timeout: float
) -> tuple[int, bytes]:
    # The server is local: never send the token through a proxy.
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    request = urllib.request.Request(url, data=body, method=method, headers=headers)
    try:
        with opener.open(request, timeout=timeout) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as e:
        with e:
            return e.code, e.read()


class Client:
    """One agent's connection. The server knows the agent only by its token.

    Content is hashed here (SHA-256) and only the hash is sent, unless
    ``send_content=True``.
    """

    def __init__(
        self,
        url: str = "http://127.0.0.1:8787",
        token: str | None = None,
        token_file: str | None = None,
        *,
        send_content: bool = False,
        timeout: float = 30.0,
        transport: Transport | None = None,
    ) -> None:
        if (token is None) == (token_file is None):
            raise ValueError("pass exactly one of `token` or `token_file`")
        if token_file is not None:
            with open(os.path.expanduser(token_file), encoding="utf-8") as f:
                token = f.read()
        assert token is not None
        self._token = token.strip()
        if not self._token:
            raise ValueError("the token is empty")
        self.url = url.rstrip("/")
        self.send_content = send_content
        self.timeout = timeout
        self._transport = transport or _urllib_transport

    def __repr__(self) -> str:
        return f"Client(url={self.url!r})"

    # -- transport -------------------------------------------------------

    def _call(self, method: str, path: str, body: dict[str, Any] | None = None) -> Any:
        headers = {"Authorization": "Bearer " + self._token}
        data = None
        if body is not None:
            data = json.dumps(body).encode()
            headers["Content-Type"] = "application/json"
        try:
            status, raw = self._transport(method, self.url + path, headers, data, self.timeout)
        except Exception as e:  # any transport failure: the check did not happen
            raise TrustGraphError("unreachable", f"cannot reach {self.url}: {e}") from None
        try:
            answer = json.loads(raw) if raw else None
        except ValueError:
            answer = None
        if 200 <= status < 300:
            if answer is None:
                raise TrustGraphError("invalid_response", "the server's answer is not JSON", status)
            return answer
        error = answer.get("error") if isinstance(answer, dict) else None
        error = error if isinstance(error, dict) else {}
        raise TrustGraphError(
            str(error.get("code", "http_error")), str(error.get("message", "request failed")), status
        )

    def _digest(self, body: dict[str, Any], content: Any, sha256: str | None) -> dict[str, Any]:
        if (content is None) == (sha256 is None):
            raise ValueError("pass exactly one of `content` or `sha256`")
        if sha256 is not None:
            body["sha256"] = sha256
        elif self.send_content and isinstance(content, str):
            body["content"] = content
        else:
            body["sha256"] = sha256_hex(content)
        return body

    @staticmethod
    def _artifact(answer: Any) -> Artifact:
        artifact = answer.get("artifact") if isinstance(answer, dict) else None
        if not isinstance(artifact, str) or not _ID.match(artifact):
            raise TrustGraphError("invalid_response", "no artifact id in the answer")
        return Artifact(artifact)

    # -- agent routes ----------------------------------------------------

    def register_source(self, source: str, content: Any = None, sha256: str | None = None) -> Artifact:
        """Record that the agent read ``source``."""
        return self._artifact(self._call("POST", "/v1/sources", self._digest({"source": source}, content, sha256)))

    def record_output(
        self,
        inputs: Iterable[Artifact | str],
        step: str,
        content: Any = None,
        sha256: str | None = None,
    ) -> Artifact:
        """Record something the agent wrote from ``inputs`` (1 to 64 artifacts)."""
        body = {"inputs": [str(i) for i in inputs], "step": step}
        return self._artifact(self._call("POST", "/v1/outputs", self._digest(body, content, sha256)))

    def check(
        self, artifact: Artifact | str, action: str, recipients: Iterable[str] | None = None
    ) -> Decision:
        """Ask right before acting. This returns every verdict; it raises only when
        the check could not be completed."""
        names = None if recipients is None else tuple(recipients)
        body = {"artifact": str(artifact), "action": action, "recipients": None if names is None else list(names)}
        answer = self._call("POST", "/v1/check", body)
        return Decision._from_json(answer, str(artifact), action, names, self)

    def explain(self, artifact: Artifact | str) -> dict[str, Any]:
        """``{"artifact", "sources", "readers"}``; 404 ``unknown_artifact`` unless the
        agent may use every source the artifact derives from."""
        artifact = str(artifact)
        if not _ID.match(artifact):
            raise TrustGraphError("unknown_artifact", "not an artifact id")
        return self._call("GET", "/v1/artifacts/" + artifact)  # type: ignore[no-any-return]

    def policy(self) -> dict[str, Any]:
        """The active policy: ``{"policy", "revision", "digest"}``."""
        return self._call("GET", "/v1/policy")  # type: ignore[no-any-return]

    def propose(self, policy: str) -> dict[str, Any]:
        """Hand a policy change to the operator. It never changes the active policy."""
        return self._call("POST", "/v1/policy/propose", {"policy": policy})  # type: ignore[no-any-return]

    def policy_check(self, policy: str) -> dict[str, Any]:
        return self._call("POST", "/v1/policy/check", {"policy": policy})  # type: ignore[no-any-return]

    def policy_test(self, policy: str) -> dict[str, Any]:
        return self._call("POST", "/v1/policy/test", {"policy": policy})  # type: ignore[no-any-return]

    def policy_explain(self, policy: str) -> dict[str, Any]:
        return self._call("POST", "/v1/policy/explain", {"policy": policy})  # type: ignore[no-any-return]

    def policy_diff(self, new: str, old: str | None = None) -> dict[str, Any]:
        body = {"new": new} if old is None else {"old": old, "new": new}
        return self._call("POST", "/v1/policy/diff", body)  # type: ignore[no-any-return]

    # -- sessions --------------------------------------------------------

    def session(self, carry: str | None = None) -> Session:
        """A new session for this agent, optionally carrying a hand-off token."""
        from ._session import Session

        return Session(self, carry=carry)


_default: Client | None = None


def set_default_client(client: Client | None) -> None:
    """Set the client ``trustgraph.session()`` uses."""
    global _default
    _default = client


def default_client() -> Client:
    if _default is None:
        raise TrustGraphError("no_client", "no default client; call trustgraph.set_default_client(Client(...))")
    return _default

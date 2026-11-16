"""TrustGraph: what an agent read follows what it writes.

See docs/sdk-python.md in the TrustGraph repository.
"""

from ._client import Client, Transport, default_client, set_default_client
from ._decorators import action, source, wait_for_approval
from ._errors import Denied, InvalidContent, NeedsApproval, TrustGraphError
from ._session import Session, current_session, merge_tokens, session
from ._types import Artifact, Blocking, Decision

__all__ = [
    "Artifact",
    "Blocking",
    "Client",
    "Decision",
    "Denied",
    "InvalidContent",
    "NeedsApproval",
    "Session",
    "Transport",
    "TrustGraphError",
    "action",
    "current_session",
    "default_client",
    "merge_tokens",
    "session",
    "set_default_client",
    "source",
    "wait_for_approval",
]

__version__ = "0.1.0"

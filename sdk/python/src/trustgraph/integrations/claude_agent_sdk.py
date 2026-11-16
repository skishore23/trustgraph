"""Claude Agent SDK.

``Guard(client, sources={...}, actions={...}).hooks()`` returns the
``ClaudeAgentOptions(hooks=...)`` value:

- ``PreToolUse``: a mapped action is recorded and checked; anything but allow
  is ``permissionDecision: "deny"`` with the reason. (Never "ask": the SDK's
  own permission prompt would let a person approve around the operator.)
- ``PostToolUse``: a mapped source's result is registered. If that fails, the
  result is replaced (``updatedToolOutput``) and the run is stopped
  (``continue: false``), because the SDK keeps the original result when a
  PostToolUse hook fails or times out.
- ``SubagentStart`` / ``SubagentStop``: each subagent gets its own session,
  starting with what its parent had seen (its prompt came from the parent);
  when it stops, the parent receives what it saw.

Sessions are keyed by the SDK's ``session_id`` and the subagent's
``agent_id``. Tool names are matched exactly, e.g. ``mcp__drive__read``.
"""

from __future__ import annotations

import asyncio
import threading
from collections.abc import Mapping
from typing import Any

from claude_agent_sdk import HookMatcher

from .._client import Client
from .._errors import TrustGraphError
from .._session import Session
from ._common import Action, as_spec, blocked_message, check_action, not_registered_message

__all__ = ["Action", "Guard"]

Key = tuple[str, "str | None"]


class Guard:
    """Map Claude Agent SDK tool names to TrustGraph sources and actions.

    ``subagents`` maps a subagent type to its own ``Client`` (another agent of
    the policy); other subagents use ``client``.
    """

    def __init__(
        self,
        client: Client,
        sources: Mapping[str, str] | None = None,
        actions: Mapping[str, str | Action] | None = None,
        *,
        subagents: Mapping[str, Client] | None = None,
        timeout: float = 60.0,
    ) -> None:
        self.client = client
        self.sources = dict(sources or {})
        self.actions = {name: as_spec(a) for name, a in (actions or {}).items()}
        self.subagents = dict(subagents or {})
        self.timeout = timeout
        self._sessions: dict[Key, Session] = {}
        self._lock = threading.Lock()

    def session(self, session_id: str, agent_id: str | None = None, agent_type: str | None = None) -> Session:
        """The TrustGraph session for an SDK session (and subagent), created on first use."""
        key = (session_id, agent_id)
        with self._lock:
            if key not in self._sessions:
                client = self.subagents.get(agent_type or "", self.client) if agent_id else self.client
                self._sessions[key] = client.session()
            return self._sessions[key]

    def forget(self, session_id: str) -> None:
        """Drop the sessions of an SDK session that has ended."""
        with self._lock:
            for key in [k for k in self._sessions if k[0] == session_id]:
                del self._sessions[key]

    def _for(self, data: Mapping[str, Any]) -> Session:
        return self.session(data["session_id"], data.get("agent_id"), data.get("agent_type"))

    # -- hook callbacks --------------------------------------------------

    async def pre_tool_use(self, data: Any, tool_use_id: str | None, context: Any) -> dict[str, Any]:
        spec = self.actions.get(data.get("tool_name", ""))
        if spec is None:
            return {}
        try:
            s = self._for(data)
            await asyncio.to_thread(check_action, s, spec, data.get("tool_input") or {})
        except TrustGraphError as e:
            reason = blocked_message(e, spec.name)
        except Exception as e:  # fail closed
            reason = blocked_message(TrustGraphError("internal_error", repr(e)), spec.name)
        else:
            return {}
        return {
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": reason,
            }
        }

    async def post_tool_use(self, data: Any, tool_use_id: str | None, context: Any) -> dict[str, Any]:
        source = self.sources.get(data.get("tool_name", ""))
        if source is None:
            return {}
        try:
            s = self._for(data)
            await asyncio.to_thread(s.read, source, data.get("tool_response"))
        except Exception as e:
            err = e if isinstance(e, TrustGraphError) else TrustGraphError("internal_error", repr(e))
            text = not_registered_message(source, err)
            return {
                "continue_": False,
                "stopReason": text,
                "hookSpecificOutput": {"hookEventName": "PostToolUse", "updatedToolOutput": text},
            }
        return {}

    async def subagent_start(self, data: Any, tool_use_id: str | None, context: Any) -> dict[str, Any]:
        parent = self.session(data["session_id"])
        child = self._for(data)
        # A failed receive breaks the child's session: its mapped tools are refused.
        await asyncio.to_thread(child.receive, parent.handoff())
        return {}

    async def subagent_stop(self, data: Any, tool_use_id: str | None, context: Any) -> dict[str, Any]:
        child = self._for(data)
        parent = self.session(data["session_id"])
        try:
            await asyncio.to_thread(parent.receive, child.handoff())
        except TrustGraphError:
            pass  # the parent's session is now broken: its mapped tools are refused
        return {}

    def hooks(self) -> dict[str, list[HookMatcher]]:
        """The value for ``ClaudeAgentOptions(hooks=...)``. Every matcher is ``on_failure="block"``."""

        def matcher(callback: Any) -> list[HookMatcher]:
            return [HookMatcher(matcher=None, hooks=[callback], timeout=self.timeout, on_failure="block")]

        return {
            "PreToolUse": matcher(self.pre_tool_use),
            "PostToolUse": matcher(self.post_tool_use),
            "SubagentStart": matcher(self.subagent_start),
            "SubagentStop": matcher(self.subagent_stop),
        }

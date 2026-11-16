"""Claude Agent SDK.

``Guard(client, sources={...}, actions={...}).hooks()`` returns the
``ClaudeAgentOptions(hooks=...)`` value:

- ``PreToolUse``: a mapped action is recorded and checked; anything but allow
  is ``permissionDecision: "deny"`` with the reason. (Never "ask": the SDK's
  own permission prompt would let a person approve around the operator.)
- ``PostToolUse``: a mapped source's result is registered. If that fails, the
  SDK session is marked broken (every later mapped action, subagent start and
  hand-off in it is refused), the result is replaced (``updatedToolOutput``)
  and the turn is stopped (``continue: false``). The SDK may keep the original
  result in the transcript anyway, which is why the session is refused.
- ``SubagentStart`` / ``SubagentStop``: each subagent gets its own session,
  starting with what its parent had seen (its prompt came from the parent);
  when it stops, the parent receives what it saw.
- ``UserPromptSubmit``: notes the SDK session before its first tool call.

Sessions are keyed by the SDK's ``session_id`` and the subagent's
``agent_id``. Tool names are matched exactly, e.g. ``mcp__drive__read``.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
import tempfile
import threading
from collections import OrderedDict
from collections.abc import Mapping
from typing import Any

from claude_agent_sdk import HookMatcher

from .._client import Client
from .._errors import TrustGraphError
from .._session import Session, _token
from ._common import Action, as_spec, blocked_message, check_action, not_registered_message

__all__ = ["Action", "Guard"]

_RESUMED = (
    "this SDK session already holds tool results from before this guard saw it (a resumed or forked "
    "conversation, a new process, or a session evicted from memory) and no stored TrustGraph session for it; "
    "pass Guard(store=...) to keep sessions across processes"
)


def _transcript_has_tool_results(path: Any) -> bool:
    """Whether the SDK's transcript already holds a tool result. Unreadable counts as yes."""
    if not path:
        return False
    try:
        with open(path, encoding="utf-8") as f:
            for line in f:
                try:
                    entry = json.loads(line)
                except ValueError:
                    continue
                message = entry.get("message") if isinstance(entry, dict) else None
                content = message.get("content") if isinstance(message, dict) else None
                if isinstance(content, list) and any(
                    isinstance(c, dict) and c.get("type") == "tool_result" for c in content
                ):
                    return True
    except FileNotFoundError:
        return False
    except OSError:
        return True
    return False


class _Conversation:
    """One SDK session: its main session, its subagents, and whether it is refused."""

    def __init__(self, main: Session) -> None:
        self.main = main
        self.subagents: dict[str, Session] = {}
        self.active: list[str] = []  # started, not yet stopped
        self.broken: str | None = main.broken

    def sessions(self) -> list[Session]:
        return [self.main, *self.subagents.values()]

    def seen(self) -> list[str]:
        return list(dict.fromkeys(i for s in self.sessions() for i in s.seen))


class Guard:
    """Map Claude Agent SDK tool names to TrustGraph sources and actions.

    ``subagents`` maps a subagent type to its own ``Client`` (another agent of
    the policy); other subagents use ``client``. At most ``max_sessions`` SDK
    sessions are kept in memory (least recently used first out). ``store`` is a
    directory where each SDK session's token, and whether it is broken, is kept
    so that a resumed SDK session (``resume=``, a new process) starts from it;
    off by default.
    """

    def __init__(
        self,
        client: Client,
        sources: Mapping[str, str] | None = None,
        actions: Mapping[str, str | Action] | None = None,
        *,
        subagents: Mapping[str, Client] | None = None,
        timeout: float = 60.0,
        max_sessions: int = 1024,
        store: str | os.PathLike[str] | None = None,
    ) -> None:
        if max_sessions < 1:
            raise ValueError("max_sessions must be at least 1")
        self.client = client
        self.sources = dict(sources or {})
        self.actions = {name: as_spec(a) for name, a in (actions or {}).items()}
        self.subagents = dict(subagents or {})
        self.timeout = timeout
        self.max_sessions = max_sessions
        self.store = None if store is None else os.fspath(store)
        self._conversations: OrderedDict[str, _Conversation] = OrderedDict()
        self._lock = threading.Lock()

    # -- the store -------------------------------------------------------

    def _path(self, session_id: str) -> str:
        assert self.store is not None
        return os.path.join(self.store, hashlib.sha256(session_id.encode()).hexdigest() + ".json")

    def _load(self, session_id: str) -> dict[str, Any] | None:
        if self.store is None:
            return None
        try:
            with open(self._path(session_id), encoding="utf-8") as f:
                stored = json.load(f)
            if not isinstance(stored, dict):
                raise ValueError("not an object")
            return stored
        except FileNotFoundError:
            return None
        except (OSError, ValueError):
            return {"broken": "the stored TrustGraph session for this SDK session could not be read"}

    def _save(self, session_id: str, c: _Conversation) -> None:
        if self.store is None:
            return
        os.makedirs(self.store, mode=0o700, exist_ok=True)
        body = json.dumps({"token": _token(c.seen()), "broken": c.broken})
        fd, tmp = tempfile.mkstemp(dir=self.store, prefix=".tmp-")
        try:
            with os.fdopen(fd, "w", encoding="utf-8") as f:
                f.write(body)
            os.replace(tmp, self._path(session_id))
        except BaseException:
            os.unlink(tmp)
            raise

    # -- sessions --------------------------------------------------------

    def _conversation(self, session_id: str, transcript_path: Any = None) -> _Conversation:
        with self._lock:
            c = self._conversations.get(session_id)
            if c is not None:
                self._conversations.move_to_end(session_id)
                return c
        main = self.client.session()
        stored = self._load(session_id)
        if stored is not None:
            if stored.get("broken"):
                main.mark_broken(str(stored["broken"]))
            elif stored.get("token"):
                try:
                    main.receive(str(stored["token"]))  # a failure breaks `main`
                except TrustGraphError:
                    pass
            else:
                main.mark_broken("the stored TrustGraph session for this SDK session is empty")
        elif _transcript_has_tool_results(transcript_path):
            main.mark_broken(_RESUMED)
        fresh = _Conversation(main)
        with self._lock:
            c = self._conversations.setdefault(session_id, fresh)
            self._conversations.move_to_end(session_id)
            while len(self._conversations) > self.max_sessions:
                self._conversations.popitem(last=False)
        if c is fresh:
            self._save(session_id, c)
        return c

    def _break(self, session_id: str, c: _Conversation, reason: str) -> None:
        with self._lock:
            if c.broken is None:
                c.broken = reason
            for s in c.sessions():
                s.mark_broken(c.broken)
        self._save(session_id, c)

    def session(self, session_id: str, agent_id: str | None = None) -> Session:
        """The TrustGraph session for an SDK session (created on first use), or one of its subagents."""
        c = self._conversation(session_id)
        if agent_id is None:
            return c.main
        try:
            return c.subagents[agent_id]
        except KeyError:
            raise TrustGraphError(
                "unknown_subagent", f"subagent `{agent_id}` did not start under this guard; its mapped tools are refused"
            ) from None

    def forget(self, session_id: str) -> None:
        """Drop an SDK session from memory (not from the store)."""
        with self._lock:
            self._conversations.pop(session_id, None)

    def _for(self, data: Mapping[str, Any]) -> tuple[_Conversation, Session]:
        c = self._conversation(data["session_id"], data.get("transcript_path"))
        if c.broken is not None:
            raise TrustGraphError("session_broken", c.broken)
        return c, self.session(data["session_id"], data.get("agent_id"))

    # -- hook callbacks --------------------------------------------------

    async def user_prompt_submit(self, data: Any, tool_use_id: str | None, context: Any) -> dict[str, Any]:
        try:
            await asyncio.to_thread(self._conversation, data["session_id"], data.get("transcript_path"))
        except Exception:
            pass  # noted again, and refused if need be, at the first mapped tool
        return {}

    async def pre_tool_use(self, data: Any, tool_use_id: str | None, context: Any) -> dict[str, Any]:
        spec = self.actions.get(data.get("tool_name", ""))
        if spec is None:
            try:  # note the SDK session before any result reaches its transcript
                await asyncio.to_thread(self._conversation, data["session_id"], data.get("transcript_path"))
            except Exception:
                pass
            return {}
        try:
            _, s = await asyncio.to_thread(self._for, data)
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
        tool = data.get("tool_name", "")
        source = self.sources.get(tool)
        if source is None:
            return {}
        try:
            c, s = await asyncio.to_thread(self._for, data)
            await asyncio.to_thread(s.read, source, data.get("tool_response"))
            await asyncio.to_thread(self._save, data["session_id"], c)
        except Exception as e:
            err = e if isinstance(e, TrustGraphError) else TrustGraphError("internal_error", repr(e))
            try:
                c = await asyncio.to_thread(self._conversation, data["session_id"])
                reason = (
                    f"a read could not be recorded ({err.code}): the result of `{tool}` may still be in the "
                    "conversation, so this session refuses every mapped action and hand-off from now on"
                )
                await asyncio.to_thread(self._break, data["session_id"], c, reason)
            except Exception:
                pass  # the replace-and-stop below still applies
            text = not_registered_message(source, err)
            return {
                "continue_": False,
                "stopReason": text,
                "hookSpecificOutput": {"hookEventName": "PostToolUse", "updatedToolOutput": text},
            }
        return {}

    def _start(self, data: Mapping[str, Any]) -> None:
        sid, agent_id = data["session_id"], data["agent_id"]
        c = self._conversation(sid, data.get("transcript_path"))
        client = self.subagents.get(data.get("agent_type") or "", self.client)
        child = client.session()
        with self._lock:
            parent_id = data.get("parent_agent_id")
            if parent_id:
                parents = [c.subagents.get(parent_id)]
            else:
                # The hook input does not name the spawning subagent: start from the main
                # session and every subagent still running, one of which may have spawned it.
                parents = [c.main, *(c.subagents[a] for a in c.active)]
            c.subagents[agent_id] = child
            c.active.append(agent_id)
        try:
            if c.broken is not None:
                raise TrustGraphError("session_broken", c.broken)
            for parent in parents:
                if parent is None:
                    raise TrustGraphError("unknown_subagent", f"parent subagent `{parent_id}` is not known")
                child.receive(parent.handoff())
        except TrustGraphError as e:
            child.mark_broken(f"this subagent could not start from what its parent had seen ({e.code})")
            raise
        finally:
            self._save(sid, c)

    def _stop(self, data: Mapping[str, Any]) -> None:
        sid, agent_id = data["session_id"], data["agent_id"]
        c = self._conversation(sid, data.get("transcript_path"))
        with self._lock:
            child = c.subagents.get(agent_id)
            if agent_id in c.active:
                c.active.remove(agent_id)
            parent_id = data.get("parent_agent_id")
            if parent_id:
                targets = [c.subagents.get(parent_id)]
            else:  # as in _start: the main session and every subagent still running
                targets = [c.main, *(c.subagents[a] for a in c.active)]
        try:
            if child is None:
                raise TrustGraphError("unknown_subagent", f"subagent `{agent_id}` did not start under this guard")
            token = child.handoff()
            for target in targets:
                if target is None:
                    raise TrustGraphError("unknown_subagent", f"parent subagent `{parent_id}` is not known")
                try:
                    target.receive(token)  # a failure breaks that session: its mapped tools are refused
                except TrustGraphError:
                    pass
        except TrustGraphError as e:
            # What the subagent read reaches its parent through its answer: refuse the SDK session.
            self._break(sid, c, f"a subagent's reads could not be handed back ({e.code})")
            return
        if c.main.broken is not None:
            self._break(sid, c, c.main.broken)
        self._save(sid, c)

    async def subagent_start(self, data: Any, tool_use_id: str | None, context: Any) -> dict[str, Any]:
        # A failure raises, so with on_failure="block" the subagent does not start; its session is
        # broken either way, so its mapped tools are refused.
        await asyncio.to_thread(self._start, data)
        return {}

    async def subagent_stop(self, data: Any, tool_use_id: str | None, context: Any) -> dict[str, Any]:
        # Never raises: a failure to hand back marks the SDK session broken instead.
        try:
            await asyncio.to_thread(self._stop, data)
        except Exception as e:
            try:
                c = await asyncio.to_thread(self._conversation, data["session_id"])
                await asyncio.to_thread(self._break, data["session_id"], c, f"a subagent could not stop cleanly ({e!r})")
            except Exception:
                pass
        return {}

    def hooks(self) -> dict[str, list[HookMatcher]]:
        """The value for ``ClaudeAgentOptions(hooks=...)``. Every matcher is ``on_failure="block"``."""

        def matcher(callback: Any) -> list[HookMatcher]:
            return [HookMatcher(matcher=None, hooks=[callback], timeout=self.timeout, on_failure="block")]

        return {
            "UserPromptSubmit": matcher(self.user_prompt_submit),
            "PreToolUse": matcher(self.pre_tool_use),
            "PostToolUse": matcher(self.post_tool_use),
            "SubagentStart": matcher(self.subagent_start),
            "SubagentStop": matcher(self.subagent_stop),
        }

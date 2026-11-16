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
import stat
import tempfile
import threading
from collections import OrderedDict
from collections.abc import Iterable, Mapping
from typing import Any

from claude_agent_sdk import HookMatcher

from .._client import Client
from .._errors import TrustGraphError
from .._session import Session, _token, merge_tokens
from ._common import Action, as_spec, blocked_message, check_action, not_registered_message

__all__ = ["Action", "Guard"]

_RESUMED = (
    "the conversation already holds a result of `{tool}` that this guard never recorded (a resumed or forked "
    "conversation, a new process, or a session evicted from memory); pass Guard(store=...) to keep sessions "
    "across processes"
)
_UNVERIFIABLE = (
    "the transcript ({path}) cannot be read, and this guard has not seen this SDK session from its start, so "
    "it cannot tell whether the conversation holds results it never recorded"
)


def _version() -> str:
    try:
        from importlib.metadata import version

        return f"claude-agent-sdk {version('claude-agent-sdk')}"
    except Exception:
        return "claude-agent-sdk (version unknown)"


def _require(data: Mapping[str, Any], *fields: str) -> None:
    """Fail closed when the hook input lacks a field the guard reads."""
    missing = [f for f in fields if f not in data]
    if missing:
        raise TrustGraphError(
            "unsupported_framework",
            f"{_version()}: the hook input has no {', '.join(missing)}, so the guard cannot check this call",
        )


class _Absent(list):  # type: ignore[type-arg]
    """No transcript file (yet): no results, but nothing verified either."""


def _tool_results(path: Any) -> list[tuple[str, str | None]] | None:
    """The ``(tool_use_id, tool name)`` of every tool result in the transcript; an empty
    ``_Absent`` when the file does not exist; ``None`` when there is no path or the file
    cannot be read. A result whose ``tool_use`` is not found has no name."""
    if not path:
        return None
    names: dict[str, str] = {}
    results: list[str] = []
    try:
        with open(path, encoding="utf-8") as f:
            for line in f:
                try:
                    entry = json.loads(line)
                except ValueError:
                    continue
                message = entry.get("message") if isinstance(entry, dict) else None
                content = message.get("content") if isinstance(message, dict) else None
                for block in content if isinstance(content, list) else ():
                    if not isinstance(block, dict):
                        continue
                    if block.get("type") == "tool_use" and isinstance(block.get("id"), str):
                        names[block["id"]] = str(block.get("name"))
                    elif block.get("type") == "tool_result":
                        results.append(str(block.get("tool_use_id")))
    except FileNotFoundError:
        return _Absent()
    except OSError:
        return None
    return [(r, names.get(r)) for r in results]


class _Conversation:
    """One SDK session: its main session, its subagents, and whether it is refused."""

    def __init__(self, main: Session, recorded: set[str] | None = None, complete: bool = False) -> None:
        self.main = main
        self.subagents: dict[str, Session] = {}
        self.active: list[str] = []  # started, not yet stopped
        self.broken: str | None = main.broken
        self.recorded: set[str] = set(recorded or ())  # tool_use_ids of mapped source results the guard handled
        self.complete = complete  # the guard has seen this SDK session from its start

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
        writable_dirs: Iterable[str | os.PathLike[str]] = (),
    ) -> None:
        if max_sessions < 1:
            raise ValueError("max_sessions must be at least 1")
        self.client = client
        self.sources = dict(sources or {})
        self.actions = {name: as_spec(a) for name, a in (actions or {}).items()}
        self.subagents = dict(subagents or {})
        self.timeout = timeout
        self.max_sessions = max_sessions
        self.store = None if store is None else os.path.realpath(os.fspath(store))
        # Where the agent's tools can write: the store must be outside all of them.
        self.writable_dirs = [os.getcwd(), *(os.fspath(d) for d in writable_dirs)]
        try:
            self._check_store()
        except TrustGraphError as e:
            raise ValueError(e.message) from None
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
        """Merge the SDK session's record into the store: never fewer artifacts or recorded
        results than were stored, and a broken marker once broken."""
        if self.store is None:
            return
        self._check_store()
        with self._lock:
            old = self._load(session_id) or {}
            stored = old.get("token") if isinstance(old.get("token"), str) else None
            was_broken = old.get("broken")
            token = merge_tokens(
                stored,
                _token([], str(was_broken)) if was_broken else None,
                *(s.persist() for s in c.sessions()),
                _token(c.seen(), c.broken),
            )
            recorded = sorted(set(old.get("recorded") or ()) | c.recorded)
            body = json.dumps({"token": token, "recorded": recorded})
            fd, tmp = tempfile.mkstemp(dir=self.store, prefix=".tmp-")
            try:
                with os.fdopen(fd, "w", encoding="utf-8") as f:
                    f.write(body)
                os.replace(tmp, self._path(session_id))
            except BaseException:
                os.unlink(tmp)
                raise

    def _check_store(self, data: Mapping[str, Any] | None = None) -> None:
        """The store must be a 0700 directory of this user, outside every directory the
        agent's tools can write (the hook's ``cwd``, ``writable_dirs``, the transcript's)."""
        if self.store is None:
            return
        dirs = list(self.writable_dirs)
        if data is not None:
            dirs += [data.get("cwd"), os.path.dirname(str(data.get("transcript_path") or ""))]
        for d in dirs:
            if not d:
                continue
            inside = os.path.realpath(os.fspath(d))
            if self.store == inside or self.store.startswith(inside.rstrip(os.sep) + os.sep):
                raise TrustGraphError(
                    "unsafe_store",
                    f"the store {self.store} is inside {inside}, which the agent's tools can write: it could "
                    "rewrite the stored sessions; keep the store outside every path the agent can write",
                )
        if not os.path.lexists(self.store):
            os.makedirs(self.store, mode=0o700)
            os.chmod(self.store, 0o700)
        st = os.lstat(self.store)
        uid = os.getuid() if hasattr(os, "getuid") else st.st_uid
        if not stat.S_ISDIR(st.st_mode) or st.st_uid != uid or stat.S_IMODE(st.st_mode) & 0o077:
            raise TrustGraphError(
                "unsafe_store",
                f"the store {self.store} must be a directory owned by this user with mode 0700 "
                f"(it is {oct(stat.S_IMODE(st.st_mode))})",
            )

    # -- sessions --------------------------------------------------------

    def _conversation(self, session_id: str, transcript_path: Any = None, *, prompt: bool = False) -> _Conversation:
        with self._lock:
            c = self._conversations.get(session_id)
            if c is not None:
                self._conversations.move_to_end(session_id)
                return c
        main = self.client.session()
        stored = self._load(session_id)
        if stored is not None:
            fresh = _Conversation(main, set(stored.get("recorded") or ()))
            if stored.get("broken"):
                main.mark_broken(str(stored["broken"]))
            elif stored.get("token"):
                try:
                    main.receive(str(stored["token"]))  # a failure, or a broken marker, breaks `main`
                except TrustGraphError:
                    pass
            else:
                main.mark_broken("the stored TrustGraph session for this SDK session is empty")
            fresh.broken = main.broken
        else:
            results = _tool_results(transcript_path)
            # Seen from its start: a readable transcript without results, or (at a prompt) no
            # transcript file yet. No path, or one that cannot be read, verifies nothing.
            fresh = _Conversation(
                main, complete=results is not None and (not isinstance(results, _Absent) or prompt) and not results
            )
        with self._lock:
            c = self._conversations.setdefault(session_id, fresh)
            self._conversations.move_to_end(session_id)
            while len(self._conversations) > self.max_sessions:
                self._conversations.popitem(last=False)
        if c is fresh:
            self._save(session_id, c)
        return c

    def _verify(self, session_id: str, c: _Conversation, transcript_path: Any) -> None:
        """Refuse when the transcript holds a mapped source's result the guard never recorded,
        or cannot be read and the guard has not seen the SDK session from its start."""
        results = _tool_results(transcript_path)
        if results is None or (isinstance(results, _Absent) and not c.complete):
            raise TrustGraphError("history_unverifiable", _UNVERIFIABLE.format(path=transcript_path))
        for tool_use_id, name in results:
            if (name is None or name in self.sources) and tool_use_id not in c.recorded:
                reason = _RESUMED.format(tool=name or "an unknown tool")
                self._break(session_id, c, reason)
                raise TrustGraphError("session_broken", reason)

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

    def _for(self, data: Mapping[str, Any], *, verify: bool = False) -> tuple[_Conversation, Session]:
        _require(data, "session_id", "transcript_path")
        self._check_store(data)
        c = self._conversation(data["session_id"], data["transcript_path"])
        if c.broken is not None:
            raise TrustGraphError("session_broken", c.broken)
        if verify:
            self._verify(data["session_id"], c, data["transcript_path"])
        return c, self.session(data["session_id"], data.get("agent_id"))

    # -- hook callbacks --------------------------------------------------

    async def user_prompt_submit(self, data: Any, tool_use_id: str | None, context: Any) -> dict[str, Any]:
        try:
            await asyncio.to_thread(self._check_store, data)
            await asyncio.to_thread(
                self._conversation, data["session_id"], data.get("transcript_path"), prompt=True
            )
        except Exception:
            pass  # noted again, and refused if need be, at the first mapped tool
        return {}

    async def pre_tool_use(self, data: Any, tool_use_id: str | None, context: Any) -> dict[str, Any]:
        spec = self.actions.get(data.get("tool_name", ""))
        if spec is None:
            try:  # note the SDK session before any result reaches its transcript
                await asyncio.to_thread(self._check_store, data)
                await asyncio.to_thread(self._conversation, data["session_id"], data.get("transcript_path"))
            except Exception:
                pass
            return {}
        try:
            _require(data, "tool_use_id", "tool_input")
            _, s = await asyncio.to_thread(lambda: self._for(data, verify=True))
            await asyncio.to_thread(check_action, s, spec, data["tool_input"] or {})
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
            _require(data, "tool_use_id", "tool_response")
            c, s = await asyncio.to_thread(self._for, data)
            await asyncio.to_thread(s.read, source, data["tool_response"])
            with self._lock:
                c.recorded.add(str(data["tool_use_id"]))
            await asyncio.to_thread(self._save, data["session_id"], c)
        except Exception as e:
            err = e if isinstance(e, TrustGraphError) else TrustGraphError("internal_error", repr(e))
            try:
                c = await asyncio.to_thread(self._conversation, data["session_id"])
                with self._lock:
                    c.recorded.add(str(data.get("tool_use_id")))  # handled: it is what broke the session
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
        _require(data, "session_id", "agent_id", "transcript_path")
        self._check_store(data)
        sid, agent_id = data["session_id"], data["agent_id"]
        c = self._conversation(sid, data["transcript_path"])
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
            self._verify(sid, c, data["transcript_path"])  # a subagent start is a hand-off
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
        _require(data, "session_id", "agent_id")
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
        # Never raises. If the subagent's reads cannot be handed back to the main session, the
        # SDK session is marked broken; a running subagent that cannot receive them is broken itself.
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

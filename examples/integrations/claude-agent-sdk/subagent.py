"""A researcher subagent reads salaries; its parent then tries to post to engineering.

    trustgraph serve --policy examples/policies/handoff.policy.yaml --operator nina --state ~/tg-handoff
    python subagent.py --tokens ~/tg-handoff/tokens

Claude Code would call a model, so this script plays the CLI's side of the
SDK's control protocol and fires the hooks a real run would fire. In your
agent, drop `transport=` and use the same options.
"""

import argparse
import asyncio
import itertools
import json
import os

from claude_agent_sdk import ClaudeAgentOptions, ClaudeSDKClient
from claude_agent_sdk._internal.transport import Transport

import trustgraph
from trustgraph.integrations.claude_agent_sdk import Guard


class ScriptedCLI(Transport):
    """Answers initialize and sends hook callbacks, like the Claude Code CLI."""

    def __init__(self):
        self.inbox, self.waiting, self.ids, self.callbacks = asyncio.Queue(), {}, itertools.count(), {}

    async def connect(self): pass
    def is_ready(self): return True
    async def end_input(self): pass
    async def close(self): await self.inbox.put(None)

    async def write(self, data):
        for message in map(json.loads, data.splitlines()):
            if message.get("type") == "control_request":  # initialize
                self.callbacks = {e: m[0]["hookCallbackIds"][0] for e, m in message["request"]["hooks"].items()}
                await self.inbox.put({"type": "control_response", "response": {"subtype": "success",
                    "request_id": message["request_id"], "response": {"hook_on_failure_supported": True}}})
            else:
                self.waiting.pop(message["response"]["request_id"]).set_result(message["response"])

    async def read_messages(self):
        while (message := await self.inbox.get()) is not None:
            yield message

    async def hook(self, event, **data):
        rid = f"r{next(self.ids)}"
        self.waiting[rid] = asyncio.get_running_loop().create_future()
        # A new session: its transcript does not exist yet, so the guard sees it from its start.
        data = {"hook_event_name": event, "session_id": "demo", "transcript_path": "/nonexistent/demo.jsonl",
                "tool_use_id": rid, **data}
        await self.inbox.put({"type": "control_request", "request_id": rid, "request": {
            "subtype": "hook_callback", "callback_id": self.callbacks[event], "input": data, "tool_use_id": rid}})
        return (await self.waiting[rid]).get("response", {})


async def main(args):
    def client(agent):
        return trustgraph.Client(args.url, token_file=os.path.join(args.tokens, agent + ".token"))

    guard = Guard(client("planner"),
                  sources={"mcp__drive__salaries": "salaries", "mcp__drive__roadmap": "roadmap"},
                  actions={"mcp__slack__post": "post_to_engineering"},
                  subagents={"researcher": client("researcher")})
    options = ClaudeAgentOptions(hooks=guard.hooks())

    cli = ScriptedCLI()
    async with ClaudeSDKClient(options, transport=cli):
        await cli.hook("UserPromptSubmit", prompt="Plan Q3 and tell engineering.")
        sub = {"agent_id": "r1", "agent_type": "researcher"}
        await cli.hook("SubagentStart", **sub)
        await cli.hook("PostToolUse", tool_name="mcp__drive__salaries", tool_response="nina: 120k", **sub)
        await cli.hook("SubagentStop", **sub)
        print("researcher subagent read salaries and returned its notes")
        await cli.hook("PostToolUse", tool_name="mcp__drive__roadmap", tool_response="Q3: billing page")
        answer = await cli.hook("PreToolUse", tool_name="mcp__slack__post", tool_input={"text": "Q3 plan"})
        out = answer["hookSpecificOutput"]
        print(f"planner mcp__slack__post: {out['permissionDecision']}: {out['permissionDecisionReason']}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", default="http://127.0.0.1:8787")
    parser.add_argument("--tokens", default=os.path.expanduser("~/.trustgraph/serve/tokens"))
    asyncio.run(main(parser.parse_args()))

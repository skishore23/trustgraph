"""Two agents, each behind its own proxy; the researcher's reads follow its hand-off file.

    trustgraph serve --policy examples/policies/handoff.policy.yaml --operator nina --state ~/tg-handoff
    python handoff.py --tokens ~/tg-handoff/tokens
"""

import argparse
import asyncio
import os
import sys
import tempfile

from mcp import Client
from mcp.client.stdio import StdioServerParameters

HERE = os.path.dirname(os.path.abspath(__file__))


def proxy(args, agent, *flags):
    return StdioServerParameters(command=sys.executable, args=[
        "-m", "trustgraph.integrations.mcp", "--map", os.path.join(HERE, "mapping.yaml"),
        "--token-file", os.path.join(args.tokens, agent + ".token"), "--url", args.url, *flags,
        "--", sys.executable, os.path.join(HERE, "files_server.py")])


async def main(args):
    handoff = os.path.join(tempfile.mkdtemp(), "researcher.handoff")
    async with Client(proxy(args, "researcher", "--handoff-file", handoff)) as researcher:
        result = await researcher.call_tool("read_file", {"path": "hr/salaries.csv"})
        print("researcher read_file hr/salaries.csv:", result.content[0].text)
    with open(handoff) as f:
        token = f.read()
    async with Client(proxy(args, "planner", "--carry", token)) as planner:
        result = await planner.call_tool("read_file", {"path": "product/roadmap.md"})
        print("planner read_file product/roadmap.md:", result.content[0].text)
        result = await planner.call_tool("post_message", {"text": "Q3 plan"})
        print("planner post_message:", "error:" if result.is_error else "", result.content[0].text)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", default="http://127.0.0.1:8787")
    parser.add_argument("--tokens", default=os.path.expanduser("~/.trustgraph/serve/tokens"))
    asyncio.run(main(parser.parse_args()))

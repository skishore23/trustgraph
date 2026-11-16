"""A researcher agent hands off to a planner; the salary restriction follows the hand-off.

    trustgraph serve --policy examples/policies/handoff.policy.yaml --operator nina --state ~/tg-handoff
    python handoff.py --tokens ~/tg-handoff/tokens

The models are the SDK's ScriptedModel (no model API is called) so the run is
repeatable; swap in a real model and nothing else changes.
"""

import argparse
import os

from agents import Agent, function_tool, set_tracing_disabled
from agents.items import ToolCallOutputItem
from agents.testing import ScriptedModel, assistant_message, function_call

import trustgraph
from trustgraph.integrations.openai_agents import Guard

set_tracing_disabled(True)


@function_tool
def read_salaries() -> str:
    """Read the HR salary file."""
    return "nina: 120k, sam: 95k"


@function_tool
def read_roadmap() -> str:
    """Read the product roadmap."""
    return "Q3: ship the new billing page"


@function_tool
def post_to_engineering(text: str) -> str:
    """Post to the engineering channel."""
    return "posted"


def step(name, args=None):
    return [function_call(name, args or {}, call_id=name)]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", default="http://127.0.0.1:8787")
    parser.add_argument("--tokens", default=os.path.expanduser("~/.trustgraph/serve/tokens"))
    args = parser.parse_args()

    def client(agent):
        return trustgraph.Client(args.url, token_file=os.path.join(args.tokens, agent + ".token"))

    planner = Agent(name="planner", tools=[read_roadmap, post_to_engineering], model=ScriptedModel([
        step("read_roadmap"), step("post_to_engineering", {"text": "Q3 plan"}), [assistant_message("done")]]))
    researcher = Agent(name="researcher", tools=[read_salaries], handoffs=[planner], model=ScriptedModel([
        step("read_salaries"), step("transfer_to_planner")]))

    guard = Guard(
        clients={"researcher": client("researcher"), "planner": client("planner")},
        sources={"read_salaries": "salaries", "read_roadmap": "roadmap"},
        actions={"post_to_engineering": "post_to_engineering"},
    )
    guard.protect(researcher, planner)
    result = guard.run_sync(researcher, "Plan Q3 and tell engineering.")
    for item in result.new_items:
        if isinstance(item, ToolCallOutputItem):
            print(f"{item.agent.name} {item.raw_item['call_id']}: {item.output}")


if __name__ == "__main__":
    main()

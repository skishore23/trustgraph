"""Two LangGraph agents hand work to each other; the salary restriction follows the hand-off.

    trustgraph serve --policy examples/policies/handoff.policy.yaml --operator nina --state ~/tg-handoff
    python handoff.py --tokens ~/tg-handoff/tokens

The models are scripted (no model API is called) so the run is repeatable;
swap in a real chat model and nothing else changes.
"""

import argparse
import os

from langchain.agents import create_agent
from langchain_core.language_models.fake_chat_models import GenericFakeChatModel
from langchain_core.messages import AIMessage, HumanMessage, ToolMessage
from langchain_core.tools import tool
from langgraph.graph import END, START, MessagesState, StateGraph

import trustgraph
from trustgraph.integrations.langchain import Guard, agent_node


@tool
def read_salaries() -> str:
    """Read the HR salary file."""
    return "nina: 120k, sam: 95k"


@tool
def read_roadmap() -> str:
    """Read the product roadmap."""
    return "Q3: ship the new billing page"


@tool
def post_to_engineering(text: str) -> str:
    """Post to the engineering channel."""
    return "posted"


class Scripted(GenericFakeChatModel):
    def bind_tools(self, tools, **kwargs):
        return self


def script(*steps):
    return Scripted(messages=iter([
        AIMessage("", tool_calls=[{"name": name, "args": args, "id": f"{name}-{i}"}]) for i, (name, args) in enumerate(steps)
    ] + [AIMessage("done")]))


class State(MessagesState):
    trustgraph: str  # the hand-off token


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", default="http://127.0.0.1:8787")
    parser.add_argument("--tokens", default=os.path.expanduser("~/.trustgraph/serve/tokens"))
    args = parser.parse_args()

    guard = Guard(sources={"read_salaries": "salaries", "read_roadmap": "roadmap"},
                  actions={"post_to_engineering": "post_to_engineering"})
    tools = [read_salaries, read_roadmap, post_to_engineering]
    researcher = create_agent(script(("read_salaries", {})), tools, middleware=[guard.middleware()])
    planner = create_agent(script(("read_roadmap", {}), ("post_to_engineering", {"text": "Q3 plan"})),
                           tools, middleware=[guard.middleware()])

    def client(agent):
        return trustgraph.Client(args.url, token_file=os.path.join(args.tokens, agent + ".token"))

    graph = StateGraph(State)
    graph.add_node("researcher", agent_node(client("researcher"), researcher))
    graph.add_node("planner", agent_node(client("planner"), planner))
    graph.add_edge(START, "researcher")
    graph.add_edge("researcher", "planner")
    graph.add_edge("planner", END)

    out = graph.compile().invoke({"messages": [HumanMessage("Plan Q3 and tell engineering.")]})
    for m in out["messages"]:
        if isinstance(m, ToolMessage):
            print(f"{m.name}: [{m.status}] {m.content}")


if __name__ == "__main__":
    main()

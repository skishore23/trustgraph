#!/usr/bin/env python3
"""A dependency-free client for `trustgraph serve`, and a demo of the starter policy.

    trustgraph policy init
    trustgraph serve --policy trustgraph.policy.yaml --operator leo
    python3 examples/python_client.py [--tokens ~/.trustgraph/serve/tokens] [--wait]

The demo runs the starter policy's flow: the planner's salary-plus-roadmap
summary is denied for Slack, its roadmap-only summary is allowed, and the
mailer's quote needs approval. With --wait it polls until the operator
approves the quote on the operator page.

Your agent's identity is its token. Give each agent only its own token file.
"""

import argparse
import json
import os
import sys
import time
import urllib.error
import urllib.request


class TrustGraphError(Exception):
    def __init__(self, status, code, message):
        super().__init__(f"{status} {code}: {message}")
        self.status = status
        self.code = code


class TrustGraph:
    """One agent's connection. The server knows the agent only by `token`."""

    def __init__(self, token, url="http://127.0.0.1:8787"):
        self.url = url.rstrip("/")
        self.token = token.strip()
        # Loopback only: never send the token through a proxy.
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def _call(self, method, path, body=None):
        data = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request(self.url + path, data=data, method=method)
        request.add_header("Authorization", "Bearer " + self.token)
        if data is not None:
            request.add_header("Content-Type", "application/json")
        try:
            with self.opener.open(request, timeout=30) as response:
                return json.load(response)
        except urllib.error.HTTPError as e:
            try:
                error = json.load(e).get("error", {})
            except ValueError:
                error = {}
            raise TrustGraphError(e.code, error.get("code", "http_error"), error.get("message", str(e)))

    def register_source(self, source, content=None, sha256=None):
        """Record that the agent read `source`; returns the artifact id."""
        body = {"source": source}
        body.update({"content": content} if sha256 is None else {"sha256": sha256})
        return self._call("POST", "/v1/sources", body)["artifact"]

    def record_output(self, inputs, step, content=None, sha256=None):
        """Record something the agent wrote from `inputs`; returns its artifact id."""
        body = {"inputs": list(inputs), "step": step}
        body.update({"content": content} if sha256 is None else {"sha256": sha256})
        return self._call("POST", "/v1/outputs", body)["artifact"]

    def check(self, artifact, action, recipients=None):
        """Ask right before acting. Act only when the verdict is "allow"."""
        return self._call(
            "POST", "/v1/check", {"artifact": artifact, "action": action, "recipients": recipients}
        )

    def explain_artifact(self, artifact):
        return self._call("GET", "/v1/artifacts/" + artifact)

    def policy(self):
        return self._call("GET", "/v1/policy")

    def propose(self, policy_yaml):
        """Hand a policy change to the operator. It never changes the active policy."""
        return self._call("POST", "/v1/policy/propose", {"policy": policy_yaml})


def token(directory, agent):
    with open(os.path.join(directory, agent + ".token")) as f:
        return f.read()


def show(label, decision):
    print(f"{label}: {decision['verdict']} ({decision['code']})")
    print(f"  {decision['reason']}")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--url", default="http://127.0.0.1:8787")
    parser.add_argument(
        "--tokens", default=os.path.expanduser("~/.trustgraph/serve/tokens"), help="token directory"
    )
    parser.add_argument("--wait", action="store_true", help="poll until the quote is approved")
    args = parser.parse_args()

    planner = TrustGraph(token(args.tokens, "planner"), args.url)
    mailer = TrustGraph(token(args.tokens, "mailer"), args.url)

    salaries = planner.register_source("salaries", content="nina: 120k, sam: 95k")
    roadmap = planner.register_source("roadmap", content="Q3: ship the new billing page")
    plan = planner.record_output([salaries, roadmap], "headcount_plan", content="Q3 headcount plan")
    show("planner post_to_slack (salaries + roadmap)", planner.check(plan, "post_to_slack"))

    summary = planner.record_output([roadmap], "roadmap_summary", content="Q3: billing page")
    show("planner post_to_slack (roadmap only)", planner.check(summary, "post_to_slack"))

    sheet = mailer.register_source("price_sheet", content="Widget: $12")
    quote = mailer.record_output([sheet], "draft_quote", content="Dear Acme, a widget is $12.")
    decision = mailer.check(quote, "email_customer")
    show("mailer email_customer", decision)

    try:
        mailer.register_source("salaries", content="nina: 120k")
    except TrustGraphError as e:
        print(f"mailer reads salaries: refused ({e.status} {e.code})")

    if decision["verdict"] == "needs_approval":
        if not args.wait:
            print("Approve it on the operator page, then run again with --wait.")
            return 0
        print("Waiting for the operator to approve on the operator page ...")
        while decision["verdict"] == "needs_approval":
            time.sleep(2)
            decision = mailer.check(quote, "email_customer")
        show("mailer email_customer", decision)
    if decision["verdict"] == "allow":
        print("Sending the quote now: the check said allow.")
    return 0


if __name__ == "__main__":
    sys.exit(main())

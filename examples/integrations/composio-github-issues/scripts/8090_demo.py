#!/usr/bin/env python3
"""Run the admitted bank demo through Codex's authenticated MCP connections.

Requires Python 3.9+ and the configured `codex` CLI. No model turn is started by
this transport; TrustGraph owns the fresh generation. Never approves/publishes.
"""

import argparse
import fcntl
import json
import os
from pathlib import Path
import re
import selectors
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[4]
DIRECTORY = ROOT / ".trustgraph-live/8090"
ACTIVE = {"queued", "running", "revising"}


class McpConnection:
    """Use Codex's credential store without copying OAuth secrets into scripts."""

    def __init__(self, factory_server):
        config_root = Path(os.environ.get("CODEX_HOME", str(Path.home() / ".codex")))
        config = (config_root / "config.toml").read_text()
        args = ["codex", "app-server", "-c", "features.apps=false"]
        for name in re.findall(r"^\[mcp_servers\.([^\].]+)\]\s*$", config, re.M):
            name = name.strip('"')
            if name not in {factory_server, "trustgraph"}:
                args += ["-c", f"mcp_servers.{name}.enabled=false"]
        self.process = subprocess.Popen(
            args, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, cwd=ROOT,
        )
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.process.stdout, selectors.EVENT_READ)
        self.pending = b""
        self.sequence = 0
        try:
            self.rpc("initialize", {
                "clientInfo": {"name": "trustgraph-demo-transport", "version": "1"},
                "capabilities": {"experimentalApi": True},
            })
            self.send({"method": "initialized", "params": {}})
            # Ephemeral MCP connection context: no turn, model call or saved chat.
            started = self.rpc("thread/start", {
                "ephemeral": True, "cwd": str(ROOT),
                "approvalPolicy": "never", "sandbox": "read-only",
            })
            self.thread = started["thread"]["id"]
        except BaseException:
            self.close()
            raise

    def send(self, value):
        self.process.stdin.write((json.dumps(value) + "\n").encode())
        self.process.stdin.flush()

    def rpc(self, method, params, timeout=60):
        self.sequence += 1
        sequence = self.sequence
        self.send({"id": sequence, "method": method, "params": params})
        until = time.monotonic() + timeout
        while time.monotonic() < until:
            while b"\n" in self.pending:
                line, self.pending = self.pending.split(b"\n", 1)
                try:
                    value = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if value.get("id") == sequence and ("result" in value or "error" in value):
                    if "error" in value:
                        raise RuntimeError(f"Codex MCP request failed: {value['error']}")
                    return value["result"]
                if "id" in value and "method" in value:
                    self.send({"id": value["id"], "error": {
                        "code": -32601, "message": "Interactive request requires a human client",
                    }})
            if self.selector.select(max(0, until - time.monotonic())):
                chunk = os.read(self.process.stdout.fileno(), 65536)
                if not chunk:
                    raise RuntimeError("Codex MCP transport closed")
                self.pending += chunk
                if len(self.pending) > 8_000_000:
                    raise RuntimeError("Codex MCP response exceeded the demo size limit")
        raise TimeoutError("MCP reply timed out; inspect state before retrying a mutation")

    def call(self, server, tool, arguments):
        reply = self.rpc("mcpServer/tool/call", {
            "threadId": self.thread, "server": server,
            "tool": tool, "arguments": arguments,
        })
        if reply.get("isError"):
            raise RuntimeError(f"{server}/{tool} rejected the request: {text_content(reply)}")
        return reply

    def close(self):
        self.process.terminate()
        try:
            self.process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
        self.selector.close()
        self.process.stdin.close()
        self.process.stdout.close()


def text_content(reply):
    return "\n".join(c.get("text", "") for c in reply.get("content", []) if c.get("type") == "text")


def tool_value(reply):
    value = reply.get("structuredContent")
    if value is None:
        value = json.loads(text_content(reply))
    if not isinstance(value, dict) or value.get("api_error"):
        raise RuntimeError("TrustGraph rejected the request; inspect its local workspace")
    return value


def admitted_summary(reply, expected):
    match = re.search(r"^## Summary\s*\n(.*?)^## ", text_content(reply), re.M | re.S)
    if not match or match.group(1).strip() != expected:
        raise RuntimeError("8090 work-order Summary must exactly match examples/bankcase/work-order.txt")
    return expected


def save(path, value):
    temporary = path.with_suffix(".tmp")
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as out:
        json.dump(value, out, indent=2)
        out.write("\n")
        out.flush()
        os.fsync(out.fileno())
    os.replace(temporary, path)
    directory = os.open(path.parent, os.O_RDONLY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


def submit_once(connection, state, path, slot, tool, arguments):
    record = state.setdefault(slot, {})
    if record.get("id"):
        return record["id"]
    if record.get("attempted"):
        raise RuntimeError(
            f"A {slot} reply was lost. Inspect TrustGraph before resubmitting; "
            f"the unresolved attempt is recorded in {path}"
        )
    record["attempted"] = True
    save(path, state)
    result = tool_value(connection.call("trustgraph", tool, arguments))
    identifier = result.get("id", "")
    if not re.fullmatch(r"job-[A-Za-z0-9_-]+", identifier):
        raise RuntimeError("TrustGraph reply has no valid work ID; inspect the workspace")
    record.update(result)
    save(path, state)
    return identifier


def poll(connection, identifier, state, path, slot):
    until = time.monotonic() + 900
    seen = None
    while time.monotonic() < until:
        job = tool_value(connection.call("trustgraph", "trustgraph_status", {"id": identifier}))
        state[slot]["result"] = job
        save(path, state)
        progress = (job["status"], len(job.get("events", [])))
        if progress != seen:
            events = job.get("events", [])
            stage = events[-1].get("stage", "") if events else ""
            print(f"{identifier}: {job['status']} {stage}", flush=True)
            seen = progress
        if job["status"] not in ACTIVE:
            return job
        time.sleep(3)
    raise TimeoutError("Work remains active; run the same command to resume polling")


def run(connection, config, state, path):
    expected = (ROOT / "examples/bankcase/work-order.txt").read_text().strip()
    project = config["project_id"]
    order = config["work_order_id"]
    factory = config["factory_server"]
    if state and (state.get("project_id"), state.get("work_order_id")) != (project, order):
        raise RuntimeError("Saved run belongs to a different 8090 work order")
    state.update(project_id=project, work_order_id=order)
    remote = connection.call(factory, "read_item", {
        "entity_type": "work_order", "id": order, "project_id": project,
    })
    goal = admitted_summary(remote, expected)
    print(f"Read 8090 {order}; admitted task matches", flush=True)
    workspace = tool_value(connection.call("trustgraph", "trustgraph_workspace", {}))
    if workspace.get("repository") != config["github_repository"] or not workspace.get("departments"):
        raise RuntimeError("Connected TrustGraph workspace does not match the configured bank demo")
    connection.call(factory, "read_document", {
        "entity_type": "skill", "id": "work_orders_writing_rules", "project_id": project,
    })
    if not state.get("restricted"):
        connection.call(factory, "edit_item", {
            "entity_type": "work_order", "id": order, "project_id": project, "status": "In Progress",
        })
    identifier = submit_once(connection, state, path, "restricted", "trustgraph_submit_work", {
        "goal": goal, "include_risk": True,
    })
    restricted = poll(connection, identifier, state, path, "restricted")
    if not (
        restricted["status"] == "blocked" and restricted.get("artifact")
        and restricted.get("information_permitted") is False
    ):
        raise RuntimeError("Risk-context run did not produce the expected access block; inspect the local result")
    print("Risk-derived result blocked from Engineering", flush=True)
    repaired_id = submit_once(connection, state, path, "shared", "trustgraph_repair", {"id": identifier})
    shared = poll(connection, repaired_id, state, path, "shared")
    if not (
        shared["status"] == "awaiting_review" and shared.get("information_permitted")
        and shared.get("change", {}).get("checks", {}).get("passed")
        and not shared.get("violations")
    ):
        raise RuntimeError("Shared-source run is not ready for review; inspect its local result")
    if not state.get("handed_off"):
        connection.call(factory, "edit_item", {
            "entity_type": "work_order", "id": order, "project_id": project, "status": "In Review",
        })
        state["handed_off"] = True
        save(path, state)
    print(f"8090 {order}: In Review\nTests: passed\nHuman review: {shared['review_url']}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["run", "status"])
    parser.add_argument("--config", type=Path, default=DIRECTORY / "demo.json")
    args = parser.parse_args()
    config = json.loads(args.config.read_text())
    path = args.config.parent / "run.json"
    with open(args.config.parent / "run.lock", "a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        state = json.loads(path.read_text()) if path.exists() else {}
        if args.command == "status":
            print(json.dumps({
                "project_id": config["project_id"], "work_order_id": config["work_order_id"],
                "restricted_id": state.get("restricted", {}).get("id"),
                "shared_id": state.get("shared", {}).get("id"),
                "review_url": state.get("shared", {}).get("review_url"),
                "handed_off": state.get("handed_off", False),
            }, indent=2))
            return
        connection = McpConnection(config["factory_server"])
        try:
            run(connection, config, state, path)
        finally:
            connection.close()


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, TimeoutError, OSError, ValueError, KeyError) as exc:
        print(f"Demo stopped: {exc}", file=sys.stderr)
        sys.exit(1)

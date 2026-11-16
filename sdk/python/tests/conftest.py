"""Fixtures: a real `trustgraph serve` on an ephemeral port with a fresh state directory."""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import time
import urllib.request
from collections.abc import Iterator
from dataclasses import dataclass
from pathlib import Path

import pytest

import trustgraph

REPO = Path(__file__).resolve().parents[3]
POLICIES = {
    "starter": (REPO / "crates/trustgraph-policy/starter.policy.yaml", "leo"),
    "handoff": (REPO / "examples/policies/handoff.policy.yaml", "nina"),
}
_binary: Path | None = None


def binary() -> Path:
    """`TRUSTGRAPH_BIN`, or the debug build (built once per test run)."""
    global _binary
    if _binary is None:
        if os.environ.get("TRUSTGRAPH_BIN"):
            _binary = Path(os.environ["TRUSTGRAPH_BIN"])
        else:
            if shutil.which("cargo") is None:
                pytest.skip("cargo is not installed and TRUSTGRAPH_BIN is not set")
            subprocess.run(["cargo", "build", "-q", "-p", "trustgraph-cli"], cwd=REPO, check=True)
            _binary = REPO / "target/debug/trustgraph"
    return _binary


@dataclass
class Server:
    url: str
    state: Path
    process: subprocess.Popen[str]

    def client(self, agent: str, **kwargs: object) -> trustgraph.Client:
        return trustgraph.Client(self.url, token_file=str(self.state / "tokens" / f"{agent}.token"), **kwargs)  # type: ignore[arg-type]

    def approve(self, decision: str) -> None:
        """Sign in with the launch file, as docs/serve.md shows for curl, and approve."""
        html = (self.state / "operator.html").read_text()
        secret = re.search(r"#/connect/([0-9a-f]{64})", html).group(1)  # type: ignore[union-attr]
        session = _post(self.url + "/session", None, {"Authorization": "Bearer " + secret})["session"]
        _post(self.url + "/v1/operator/approve", {"decision": decision}, {"X-TrustGraph-Operator": session})

    def stop(self) -> None:
        self.process.terminate()
        self.process.wait(timeout=10)


def _post(url: str, body: object, headers: dict[str, str]) -> dict:
    data = b"" if body is None else json.dumps(body).encode()
    request = urllib.request.Request(url, data=data, method="POST", headers=dict(headers))
    if body is not None:
        request.add_header("Content-Type", "application/json")
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with opener.open(request, timeout=10) as response:
        return json.load(response)


def start(tmp: Path, policy: str = "starter") -> Server:
    path, operator = POLICIES[policy]
    shutil.copy(path, tmp / "policy.yaml")
    state = tmp / "state"
    log = tmp / "serve.log"
    with open(log, "w") as out:
        process = subprocess.Popen(
            [str(binary()), "serve", "--policy", str(tmp / "policy.yaml"), "--operator", operator,
             "--state", str(state), "--port", "0"],
            stdout=out, stderr=subprocess.STDOUT, text=True,
        )
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline and process.poll() is None:
        match = re.search(r"TrustGraph serve: (http://127\.0\.0\.1:\d+)", log.read_text())
        if match:
            return Server(match.group(1), state, process)
        time.sleep(0.05)
    process.kill()
    raise RuntimeError("trustgraph serve did not start")


@pytest.fixture
def server(tmp_path: Path) -> Iterator[Server]:
    s = start(tmp_path, "starter")
    yield s
    s.stop()


@pytest.fixture
def handoff_server(tmp_path: Path) -> Iterator[Server]:
    s = start(tmp_path, "handoff")
    yield s
    s.stop()

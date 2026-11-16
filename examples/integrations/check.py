"""Run an integration example against a fresh `trustgraph serve` and compare its
output with the ```text block in the example's README.

    python examples/integrations/check.py langgraph [--bin target/debug/trustgraph]

Exits non-zero when the output differs, so the documented output cannot drift.
"""

import argparse
import difflib
import os
import re
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
SCRIPTS = {"langgraph": "handoff.py", "openai-agents": "handoff.py", "claude-agent-sdk": "subagent.py"}


def documented(example: str) -> list[str]:
    readme = (HERE / example / "README.md").read_text()
    blocks = re.findall(r"```text\n(.*?)```", readme, re.S)
    if len(blocks) != 1:
        sys.exit(f"{example}/README.md must have exactly one ```text block, found {len(blocks)}")
    return blocks[0].strip().splitlines()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("example", choices=sorted(SCRIPTS))
    parser.add_argument("--port", type=int, default=0, help="the server's port (default: any free one)")
    parser.add_argument("--bin", default=os.environ.get("TRUSTGRAPH_BIN", str(REPO / "target/debug/trustgraph")))
    args = parser.parse_args()

    with tempfile.TemporaryDirectory() as tmp:
        state, log = Path(tmp) / "state", Path(tmp) / "serve.log"
        with open(log, "w") as out:
            server = subprocess.Popen(
                [args.bin, "serve", "--policy", str(REPO / "examples/policies/handoff.policy.yaml"),
                 "--operator", "nina", "--state", str(state), "--port", str(args.port)],
                stdout=out, stderr=subprocess.STDOUT,
            )
        try:
            url = None
            deadline = time.monotonic() + 30
            while url is None and time.monotonic() < deadline and server.poll() is None:
                match = re.search(r"TrustGraph serve: (http://127\.0\.0\.1:\d+)", log.read_text())
                url = match.group(1) if match else time.sleep(0.05)
            if url is None:
                sys.exit("trustgraph serve did not start:\n" + log.read_text())
            run = subprocess.run(
                [sys.executable, str(HERE / args.example / SCRIPTS[args.example]),
                 "--url", url, "--tokens", str(state / "tokens")],
                capture_output=True, text=True, timeout=120,
            )
        finally:
            server.terminate()
            server.wait(timeout=10)

    if run.returncode != 0:
        sys.exit(f"{args.example} failed ({run.returncode}):\n{run.stdout}{run.stderr}")
    expected, actual = documented(args.example), run.stdout.strip().splitlines()
    if expected != actual:
        sys.stdout.writelines(difflib.unified_diff(
            [line + "\n" for line in expected], [line + "\n" for line in actual], "README.md", "output"))
        return 1
    print(f"{args.example}: output matches README.md ({len(actual)} lines)")
    return 0


if __name__ == "__main__":
    sys.exit(main())

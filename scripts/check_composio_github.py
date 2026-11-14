#!/usr/bin/env python3
"""Read-only Composio preflight: verify a GitHub account and pin the issue tool."""
import argparse
import json
import os
from pathlib import Path
import re
import stat
import sys
import urllib.request
from trustgraph_actions import NoRedirect


def private_key(path):
    metadata = path.lstat()
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_mode & 0o077 or metadata.st_size > 512:
        raise ValueError("Composio key must be a private regular file with owner-only permissions")
    key = path.read_text().strip()
    if not key or any(not 33 <= ord(c) <= 126 for c in key):
        raise ValueError("Invalid Composio key file")
    return key


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--account", required=True, help="Composio connected GitHub account ID")
    parser.add_argument("--repository", required=True, help="GitHub owner/repository")
    parser.add_argument("--directory", type=Path, default=Path(".trustgraph-live/issue-actions-live"))
    parser.add_argument("--output", type=Path, default=Path("trustgraph-actions-live.local.json"))
    args = parser.parse_args()
    if not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", args.account):
        raise ValueError("Invalid connected account ID")
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", args.repository):
        raise ValueError("Use an explicit owner/repository")
    key = os.environ.get("COMPOSIO_API_KEY") or private_key(args.directory / "composio-key")
    if not key or len(key) > 512 or any(not 33 <= ord(c) <= 126 for c in key):
        raise ValueError("Invalid Composio key")
    opener = urllib.request.build_opener(NoRedirect)

    def get(path):
        request = urllib.request.Request("https://backend.composio.dev/api/v3.1/" + path, headers={"x-api-key": key, "Accept": "application/json"})
        with opener.open(request, timeout=25) as response:
            return json.load(response)

    account = get("connected_accounts/" + args.account)
    if account.get("status") != "ACTIVE" or account.get("toolkit", {}).get("slug", "").lower() != "github":
        raise ValueError("The account is not an active GitHub connection")
    tool = get("tools/GITHUB_CREATE_AN_ISSUE?version=latest")
    version = tool.get("version", "")
    if not re.fullmatch(r"\d{8}_\d{2}", version):
        raise ValueError("Composio did not return a dated tool version; keep preview mode")
    schema = tool.get("input_parameters", {})
    properties = schema.get("properties", schema)
    expected = {"owner", "repo", "title", "body"}
    required = set(schema.get("required", [])) if isinstance(schema.get("required", []), list) else set()
    required.update(name for name, value in properties.items() if isinstance(value, dict) and value.get("required") is True)
    if not expected.issubset(properties) or not required.issubset(expected):
        raise ValueError("Issue tool schema differs from this adapter; keep preview mode until it is updated")
    config = {"directory": str(args.directory.resolve()), "github_repository": args.repository,
              "mode": "live", "connected_account_id": args.account, "user_id": account.get("user_id"),
              "tool_version": version, "agent_public_url": None}
    fd = os.open(args.output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w") as output:
        json.dump(config, output, indent=2)
        output.write("\n")
    print(f"Read-only checks passed. Pinned issue tool {version}. Configuration: {args.output.resolve()}")
    print("No issue was created. Start the gateway with --config pointing at this file when ready.")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError) as error:
        print(f"Composio preflight stopped: {error}", file=sys.stderr)
        sys.exit(1)

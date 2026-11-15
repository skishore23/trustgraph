#!/usr/bin/env python3
"""Native 8090 agent client: propose, inspect and revise exact GitHub issue actions."""
import argparse
import json
import os
from pathlib import Path
import sys
import urllib.error
import urllib.parse
import urllib.request


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, file, code, message, headers, new_url):
        return None


def endpoint(value):
    parsed = urllib.parse.urlsplit(value)
    if (parsed.scheme != "https" and not (
        parsed.scheme == "http" and parsed.hostname in {"127.0.0.1", "localhost", "::1"}
    )) or not parsed.hostname or parsed.username or parsed.password or parsed.path not in {"", "/"} or parsed.query or parsed.fragment:
        raise ValueError("TRUSTGRAPH_ACTIONS_URL must be an HTTPS origin or local HTTP origin")
    return value.rstrip("/")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", default=os.environ.get("TRUSTGRAPH_ACTIONS_URL", "http://127.0.0.1:8085"))
    parser.add_argument("--token-file", type=Path)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("list")
    status = commands.add_parser("status")
    status.add_argument("id")
    propose = commands.add_parser("propose")
    propose.add_argument("--file", type=Path, required=True)
    revise = commands.add_parser("revise")
    revise.add_argument("id")
    revise.add_argument("--file", type=Path, required=True)
    args = parser.parse_args()
    base = endpoint(args.url)
    token = args.token_file.read_text().strip() if args.token_file else os.environ.get("TRUSTGRAPH_AGENT_TOKEN", "")
    if len(token) != 64 or any(c not in "0123456789abcdefABCDEF" for c in token):
        raise ValueError("provide TRUSTGRAPH_AGENT_TOKEN through workspace secrets, or use a local --token-file")
    path = "/api/actions"
    body = None
    if args.command in {"status", "revise"}:
        path += "/" + urllib.parse.quote(args.id, safe="")
    if args.command == "revise":
        path += "/revise"
    if args.command in {"propose", "revise"}:
        body = json.dumps(json.loads(args.file.read_text())).encode()
    request = urllib.request.Request(base + path, data=body, headers={
        "Authorization": "Bearer " + token,
        "Content-Type": "application/json",
        "Accept": "application/json",
        "ngrok-skip-browser-warning": "1",
    })
    try:
        with urllib.request.build_opener(NoRedirect).open(request, timeout=40) as response:
            result = json.load(response)
    except urllib.error.HTTPError as error:
        raise RuntimeError(f"TrustGraph returned HTTP {error.code}; inspect the request and current status") from error
    except (TimeoutError, urllib.error.URLError) as error:
        raise RuntimeError("Reply unavailable. Inspect list/status before resubmitting a proposal or revision.") from error
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, RuntimeError) as error:
        print(f"Action client stopped: {error}", file=sys.stderr)
        sys.exit(1)

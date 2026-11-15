"""Read-only Composio Connect discovery. The write adapter lives in Rust."""
import hashlib
import json
import urllib.request
from trustgraph_actions import NoRedirect

ISSUE_TOOL = "GITHUB_CREATE_AN_ISSUE"
LIMIT = 1_048_576


def schema_hash(schema):
    return hashlib.sha256(json.dumps(schema, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()).hexdigest()


def read_sse_result(stream, identifier, limit=LIMIT):
    """Return the JSON-RPC message with ``identifier`` from a bounded SSE body.

    An event ends at a blank line or at end of stream, so a final event without
    a trailing blank line is still read."""
    consumed, event = 0, []

    def finish():
        item = json.loads("\n".join(event))
        return item if isinstance(item, dict) and item.get("id") == identifier else None

    while True:
        line = stream.readline(limit + 1 - consumed)
        consumed += len(line)
        if consumed > limit:
            raise ValueError("MCP response too large")
        if not line:
            if event and (result := finish()) is not None:
                return result
            raise ValueError("No bounded MCP response")
        if not line.strip():
            if event:
                result = finish()
                event = []
                if result is not None:
                    return result
        elif line.startswith(b"data:"):
            event.append(line[5:].decode().removeprefix(" ").rstrip("\r\n"))


class Connect:
    def __init__(self, key):
        self.opener = urllib.request.build_opener(NoRedirect)
        self.headers = {"x-consumer-api-key": key, "Content-Type": "application/json",
                        "Accept": "application/json, text/event-stream"}
        self.sequence = 0
        init = self.rpc("initialize", {"protocolVersion": "2025-03-26", "capabilities": {},
                                      "clientInfo": {"name": "trustgraph-preflight", "version": "1"}})
        if init.get("protocolVersion") != "2025-03-26":
            raise ValueError("Unsupported Connect MCP protocol")
        self.headers["MCP-Protocol-Version"] = init["protocolVersion"]
        with self.post({"jsonrpc": "2.0", "method": "notifications/initialized"}):
            pass

    def post(self, body):
        request = urllib.request.Request("https://connect.composio.dev/mcp",
                                         data=json.dumps(body).encode(), headers=self.headers)
        return self.opener.open(request, timeout=40)

    def rpc(self, method, params):
        self.sequence += 1
        identifier = self.sequence
        with self.post({"jsonrpc": "2.0", "id": identifier, "method": method, "params": params}) as response:
            session = response.headers.get("Mcp-Session-Id")
            if session:
                self.headers["Mcp-Session-Id"] = session
            if "text/event-stream" in response.headers.get("Content-Type", ""):
                result = read_sse_result(response, identifier)
            else:
                raw = response.read(LIMIT + 1)
                if len(raw) > LIMIT:
                    raise ValueError("MCP response too large")
                result = json.loads(raw)
        if result.get("jsonrpc") != "2.0" or result.get("id") != identifier or "error" in result:
            raise ValueError("Connect MCP request failed")
        return result["result"]

    def call(self, name, arguments):
        if name not in {"COMPOSIO_SEARCH_TOOLS", "COMPOSIO_GET_TOOL_SCHEMAS", "COMPOSIO_MULTI_EXECUTE_TOOL"}:
            raise ValueError("Unsupported preflight tool")
        if name == "COMPOSIO_MULTI_EXECUTE_TOOL" and any(
                tool.get("tool_slug") != "GITHUB_GET_A_REPOSITORY" for tool in arguments.get("tools", [])):
            raise ValueError("Preflight permits repository reads only")
        result = self.rpc("tools/call", {"name": name, "arguments": arguments})
        if result.get("isError"):
            raise ValueError("Connect MCP tool failed")
        value = result.get("structuredContent")
        if value is None:
            for block in result.get("content", []):
                if block.get("type") == "text":
                    try:
                        value = json.loads(block["text"])
                        break
                    except (ValueError, KeyError):
                        pass
        if not isinstance(value, dict) or value.get("successful") is not True:
            raise ValueError("Connect MCP returned an unsuccessful result")
        return value["data"]


def preflight(key, repository, account_id=None):
    client = Connect(key)
    search = client.call("COMPOSIO_SEARCH_TOOLS", {
        "queries": [{"use_case": "Create a GitHub issue after human approval; first verify repository access without changes",
                     "known_fields": f"repository: {repository}"}], "session": {"generate_id": True}})
    accounts = {a["id"]: a for toolkit in search["toolkit_connection_statuses"]
                if toolkit.get("toolkit") == "github" and toolkit.get("has_active_connection")
                for a in toolkit.get("accounts", []) if a.get("status") == "ACTIVE"}
    if account_id is None:
        if len(accounts) != 1:
            raise ValueError("Choose an account using --account; exactly one active GitHub connection is needed for automatic selection")
        account_id = next(iter(accounts))
    if account_id not in accounts:
        raise ValueError("Configured GitHub connection is not active in Connect")
    session = search["session"]["id"]
    schemas = client.call("COMPOSIO_GET_TOOL_SCHEMAS", {
        "tool_slugs": [ISSUE_TOOL], "include": ["input_schema"], "session_id": session})
    schema = schemas["tool_schemas"][ISSUE_TOOL]["input_schema"]
    expected = {"owner", "repo", "title", "body"}
    if not expected.issubset(schema.get("properties", {})) or not set(schema.get("required", [])).issubset(expected):
        raise ValueError("Unsupported issue schema")
    owner, repo = repository.split("/")
    response = client.call("COMPOSIO_MULTI_EXECUTE_TOOL", {
        "tools": [{"tool_slug": "GITHUB_GET_A_REPOSITORY", "account": account_id,
                   "arguments": {"owner": owner, "repo": repo}}],
        "session_id": session, "sync_response_to_workbench": False, "current_step": "VERIFY_REPOSITORY"})
    results = response["results"]
    if len(results) != 1 or results[0].get("tool_slug") != "GITHUB_GET_A_REPOSITORY":
        raise ValueError("Unverified repository response")
    result = results[0]["response"]
    data = result.get("data", {})
    if result.get("successful") is not True or data.get("full_name", "").lower() != repository.lower() or data.get("has_issues") is not True or data.get("archived") is not False:
        raise ValueError("Repository is unavailable, archived or has issues disabled")
    return {"connected_account_id": account_id, "user_id": None, "tool_version": None,
            "composio_transport": "connect_mcp", "tool_schema_hash": schema_hash(schema)}

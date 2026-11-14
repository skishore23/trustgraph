# Using TrustGraph alongside 8090 Software Factory

## The usable integration today

Connect **the same coding agent to two MCP servers**. 8090 supplies project/work-order context; TrustGraph supplies the local plan compiler and review workflow. This requires no changes inside 8090 and does not route its credentials through TrustGraph.

8090 publishes a coding-agent connection for reading project context and work orders. Its setup lives in the project's **Settings → Connections → Coding Agent**. Copy the project-specific configuration from there and complete its browser sign-in. The published endpoint is `https://api.factory.8090.dev/v2/mcp`, scoped with `X-Sofa-Mcp-Project-Id`. Use the generated values rather than guessing a project ID. [8090 coding-agent connection](https://docs.8090.ai/connections/connect-coding-agent).

Add the separate TrustGraph stdio entry described in [the MCP guide](mcp.md) without replacing the 8090 entry. Verify that the client can discover both sets of tools. Only the TrustGraph side has been exercised in this repository; no 8090 account has been connected or modified here.

## A concrete trial

Give the agent this instruction:

> Read the selected 8090 Work Order and its linked Requirements/Blueprints through the existing Software Factory connection. Keep that real project material in its authorized context; do not copy it into TrustGraph's synthetic sources. Then use TrustGraph's available consulting case to demonstrate the gate: simulate the blocked client memo, explain the conflict, propose and repair the plan, and prepare a shareable memo for human review. Return the local review link and stop. Treat this as integration plumbing and a synthetic permission test, not as authorization to publish or change the Work Order.

The acquisition trial tests that the coding agent can use both services in one workflow. It does **not** test 8090 document permissions. After the partner reviews locally, the agent can inspect `trustgraph_status`. Updating an 8090 work order with that result is a separate action; authorize its destination and content explicitly before sending it. This implementation does not send anything to 8090.

For eventual actual factory use, a useful first controlled artifact is a release-readiness memo or architecture decision derived from project requirements, internal incident evidence and client-shared specifications. The source import must establish which evidence may inform which deliverable, preserve exact versions and capture the generator's complete context. An agent's assertion that a source is shareable is insufficient. The current fixture cannot substitute for that import.

## Connecting from 8090's hosted agent

8090 also documents MCP catalog connections under **Settings → Connections → MCP Servers**, with tool permissions. Its public guide does not establish whether an arbitrary custom URL can be registered in a particular account. Confirm that capability in the account before planning a direct hosted-agent trial. [8090 MCP servers](https://docs.8090.ai/connections/mcp-servers).

That route also needs a reachable HTTPS endpoint with qualified authentication and a safe reviewer-access path. A cloud agent cannot use this laptop's `127.0.0.1` endpoint. The delivered bridge intentionally rejects public upstream URLs, and the app binds to loopback. No tunnel or public deployment is created here. Do not expose this demo's fixed reviewer identities as an internet-facing approval system.

8090 tool permissions control invocation; they do not replace TrustGraph approval of an exact generated artifact and recipient set. For a direct integration, keep preview/analyze/status separate from proposal/preparation actions, and retain human content approval in TrustGraph. TrustGraph does not wrap or block arbitrary 8090 tools simply because both servers are connected.

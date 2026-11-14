# Agent-facing MCP integration

The user wants to try TrustGraph from an agent and integrate it with 8090 Software Factory. Build an official Rust SDK MCP transport over the existing runtime, preserving the consulting demo and reviewer UI.

## Approach

A second stdio process opening the journal would compete with the web app's exclusive writer. A remote deployment requires real identity and network qualification. Use one running app with an authenticated loopback Streamable HTTP /mcp endpoint, plus a stdio bridge that connects to it. Both transports use rmcp 3.10.0, currently published on crates.io and requiring Rust 1.88 (below our 1.90 floor). Reuse the existing API router in-process with fixed demo-agent identity so validations and runtime effects stay identical. Do not create another policy implementation.

Expose seven tools: workspace context, simulate selection, propose typed plan, analyze saved plan, repair saved plan, prepare exact plan/snapshot with idempotency key, and review status. Tools cannot supply an identity, approve, change ACLs, publish, choose arbitrary API routes or access arbitrary files/URLs. Model-visible tool results include structured JSON, useful blocked/conflict errors and a reviewer path. Human approval remains in the existing local reviewer app. Tool annotations accurately distinguish preview/read operations from additive state changes.

Generate a random local capability token in the private data directory, persist it owner-readable only, and require it on every /mcp request. Reject non-loopback Host and foreign Origin. Scope the token to the fixed synthetic agent, never to a reviewer. Do not log the token. The stdio bridge reads only its explicitly configured token file; stdout contains protocol messages only. Its upstream URL is restricted to a loopback /mcp endpoint with no credentials, query or fragment; disable redirects/proxies and do not retry ambiguous mutations. Use official SDK initialization and error handling.

8090's published MCP page documents catalog connections and per-tool permissions, but does not establish custom-server registration. Therefore provide a concrete companion-agent setup: one coding agent connects to 8090 using its own supported connection and to TrustGraph using our stdio configuration. For direct hosted 8090 use, document the remaining custom-server availability, reachable HTTPS and identity requirements; do not claim it is connected or expose localhost publicly. Do not modify user-wide agent configurations or publish data to 8090 during this task.

Current source documents and generation remain synthetic. Connecting MCP does not import 8090 documents, inherit its ACLs, capture external model memory, or enforce unrelated tools. Include this limitation and an honest trial prompt.

## Validation

Use real MCP messages to test initialization/tool discovery, metadata, stateless blocked preview, proposal/repair, stale and blocked preparation, exact generation, human-only approval, status and delivery visibility, unknown tools and identity injection. Exercise the actual stdio binary as a client child process against the same running app. Check auth, Host/Origin, token persistence/permissions and bridge URL validation. Run full workspace checks and preserve existing journals.

# Composio / GitHub qualification contract

Status: **not implemented or live-qualified as the publisher**. The current server always uses `LocalPublisher`. No Composio credentials, GitHub branch-write tool or raw publication tool is exposed through TrustGraph MCP. The user will supply service details later.

Use the [official proxy contract](https://docs.composio.dev/reference/v3/api-reference/tools/postToolsExecuteProxy) with an explicit connected-account ID and user scope, or a dated, inspected GitHub toolkit schema. The proxy endpoint supports authenticated third-party HTTP calls and a response carrying the upstream status/data. That transport capability is not itself a governance gate.

The adapter must accept only an approved typed patch payload and an allowlisted repository/destination. The approval context must cover base commit, full file paths/contents, PR title/body and intended audience. Resolve current repository readers independently; if membership is incomplete or does not match the policy, stop before any write.

After current permission and approval checks, reserve an operation durably, then perform the bounded GitHub sequence: confirm base commit, create a deterministic operation branch, write exactly the approved tree/commit, and create a **draft** PR. Branch writes cannot precede this gate. A generated branch already pushed with the agent's credentials does not satisfy the boundary.

Do not assume the provider's POST is idempotent. Once a request may have been accepted, record an unknown outcome rather than blindly resending. Reconciliation must use read-only calls to verify the operation's repository, branch, base/head commits, exact changed file bytes and draft PR. A matching text marker alone is insufficient. Partial branch writes without a confirmed PR are not a completed publication. Do not automatically delete uncertain remote objects.

Required contract tests: success; wrong account/repository; changed base; different file bytes after approval; changed destination or policy; failure before first write; timeout after branch/commit/PR acceptance; app crash before outcome journaling; read-only reconciliation; no second mutation after an ambiguous reply. Then qualify the exact same paths against a dedicated live repository and retain sanitized receipts.

The existing `Publisher` interface is synchronous. A network implementation must run through a bounded worker owned by the coordinator, not introduce an unawaited async dispatch or call a blocking HTTP runtime directly inside an Axum task. Real model generation and production reviewer identity remain separate prerequisites. This document is a concrete next-stage contract, not a claim that only an API key remains.

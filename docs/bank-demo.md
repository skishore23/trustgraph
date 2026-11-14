# Bank demo: one agent, two departments

This is a fictional payment-reconciliation change with synthetic source documents. The implemented default app is the bank workspace. Consulting and Northstar fixtures remain selectable without deleting their data.

## Two-minute walkthrough

1. Open the simulator as **Ravi** (internal ID `arun`), the Risk owner. All three evidence documents are visible. Select the Engineering repository. The compiler blocks the plan and identifies Engineering recipients who cannot read the Risk investigation. A proposed patch need not quote a source for that dependency to matter.
2. Choose **Keep the audience. Use shareable evidence.** The resulting plan uses the Engineering requirements, approved specification and recorded instructions in a fresh local fixture context. It does not edit or reuse the restricted output. The versions are not claimed equivalent.
3. Prepare the patch for review. The artifact contains proposed Rust code and tests, exact recorded inputs and an explicit `local_fixture` generator marker. These are real tracked artifact bytes, but not a live coding-agent generation or a full bank implementation. The generator does not claim the proposed tests have run.
4. Switch to **Maya**, the Engineering reviewer. The original Risk-dependent artifact/plan is not accessible to her; the repaired patch is. Inspect its sources and proposed destination. Human approval concerns those exact bytes and that audience. The current publisher sends only to the local simulator.
5. In Policy studio as Ravi (internal ID `arun`), run the five cases, inspect both stale-approval decisions, save a policy with a new ID/approval lifetime and activate it. The activation changes the context, so earlier approvals cannot authorize the new policy snapshot.

For a clean repeated run, use a fresh named data directory or key; Reset changes selection, not history. For the older consulting workspace, use `serve --scenario consulting --data .trustgraph-consulting --port 8083` and download its own MCP configuration.

## Coding-agent / 8090 trial

Keep the local app running and add the downloaded MCP configuration to the coding agent. If using 8090, keep its separate existing MCP connection. Suggested instruction:

> Inspect TrustGraph's bank workspace and policy versions. Propose a second explicit policy with a new ID and a five-minute approval lifetime, preserving the three mandatory rules and authoritative readers. Run all five policy cases and explain their actual evidence. Simulate all three sources going to Engineering, identify the conflict, propose and repair that plan using fresh context, and prepare the repaired patch with the current digests and a stable key. Return its local review link. Stop before activation, branch push or PR creation. Do not copy real 8090 project data into the synthetic fixture.

The MCP integration is implemented. Its ten tools include policy list/propose/test; it exposes no policy activation, human approval or raw publication operation. The coding agent can exercise this contract now, but its own external conversations/model inputs are not captured by the local fixture renderer.

## Remaining live boundary

Real generation needs a broker-owned isolated model/tool context that records every actual supplied input and separately authorizes the model provider as a data recipient. Real GitHub delivery needs one controlled publisher that gates **both branch writes and draft PR creation** after approval. A Composio GitHub connection must not be handed to the coding agent as a second unrestricted route. Real repository membership and source-use rights must be authoritative, not inferred from a group label or an agent assertion.

The next integration needs the promised Hydra endpoint/namespace/token, Composio connected-account credentials and a dedicated allowed test repository. Nothing in this demo has pushed a bank patch or opened a real PR. Checking this project's source into GitHub is separate from implementing that guarded publisher.

For the guided three-chapter presentation and reset instructions, use the [story demo](bank-story-demo.md). Its in-memory sandbox is deliberately separate from this configurable, persistent agent workflow.

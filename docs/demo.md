# Demo walkthrough

Start with `cargo run --locked -p trustgraph-cli -- story` and open
[localhost:9081](http://127.0.0.1:9081/). No sign-in is needed. Keep work item
**TG-104: build a headcount planning dashboard for Engineering** throughout.

1. **Company.** HR owns compensation, Finance owns budgets, Engineering builds
   the dashboard. Directory and separately shared planning data have broader
   access. The same agent can help each department without erasing boundaries.
2. **Policy.** Show the access matrix and YAML. Explain source readers,
   destination audience, authorized reviewers and approval lifetime. Ask “Who can
   access this output?” Review cannot override a source restriction.
3. **Simulator.** Select **Salary + budget data → Engineering**: access is blocked.
   Select **Salary + budget data → HR**: HR has access to those sources. Select
   **Shared planning data → Engineering**, set minutes since approval to 20, and
   compare the current 30-minute rule with a proposed 15-minute rule. The shorter
   rule makes the approval expire. Repeat unchanged: the result ID matches.
   **Apply policy changes** is a separate step after testing.
4. **Enforce.** Generate the dashboard with private context. Inspect the conflict
   and its source-to-output path. Start a fresh run from shared planning data.
   The old draft is excluded. Inspect the new artifact, approve its exact version
   and record its release.
5. **Revision.** Change the actual artifact. The old approval is stale and the new
   version needs review. The previous release remains in history.

The guided story uses fixed source/output content, in-memory state and a local
publisher. Policy compilation, provenance, exact approval and decision checks use
the shared Rust engine. Restart clears only that session. It does not call Codex,
HydraDB or GitHub.

For the technical handoff, show [the integration guide](agent-integration.md):
calling agent → MCP/HTTP → controlled fresh Codex generation → Rust decision →
human review → publication gate. Use the separate [live workspace](development.md)
for real generation, tests, Hydra evidence and a GitHub draft PR. Its human
operator capability protects actual publication; it is not needed for this
presentation flow.

The [technical explainer](../media/trustgraph-technical-explainer.mp4)
([captions](../media/trustgraph-technical-explainer.srt)) covers controlled inputs,
HydraDB evidence, inherited access, exact approval and durable publication. The
[story explainer](../media/trustgraph-explainer.mp4)
([captions](../media/trustgraph-explainer.srt)) introduces the dashboard example.

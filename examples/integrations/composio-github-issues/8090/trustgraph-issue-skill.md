---

description: Use when a work order needs a GitHub issue created through TrustGraph with human approval of its exact content and destination.

---

# TrustGraph issue approval

Use the native 8090 workspace shell. Do not start Codex or another generation
agent for this workflow. Read the owning Work Order and applicable project
skills first. Follow the Work Orders Writing Rules before changing its status.

1. Inspect the workspace and locate `examples/integrations/composio-github-issues/scripts/trustgraph_actions.py` in the
   TrustGraph checkout. If no setup repository is connected, follow the project's
   Workspace Setup skill and report that missing connection. Do not invent a
   working workspace or claim a reload succeeded without checking it.
2. The workspace needs `TRUSTGRAPH_ACTIONS_URL` (the HTTPS origin of the agent API)
   and `TRUSTGRAPH_AGENT_TOKEN` (its restricted capability). Request missing
   secrets using the workspace secret workflow. Never ask for secret values in
   chat or write them into the repository. Apply them through the normal workspace
   reload procedure. Keep `COMPOSIO_API_KEY` exclusively in the TrustGraph gateway.
3. Run `python3 examples/integrations/composio-github-issues/scripts/trustgraph_actions.py list`. Confirm the configured GitHub
   repository and whether the gateway is in `preview` or `live` mode. Preview
   records a local receipt and never creates an actual issue.
4. Write a JSON proposal like `examples/integrations/composio-github-issues/8090/github-issue.json`, using the real
   project ID and Work Order number. Choose a stable `key` for this proposed
   action. Limit the title and body to material the human reviewer may inspect.
   Submitting this action does not capture your prior conversation or prove its
   source permissions. Do not claim otherwise.
5. Submit once with `python3 examples/integrations/composio-github-issues/scripts/trustgraph_actions.py propose --file proposal.json`.
   Record the returned ID, revision and mode. If a reply is lost, inspect list/status
   before resubmitting; reuse the same key and exact bytes for the same proposal.
6. Return the ID to the human operator and ask them to open their local TrustGraph
   issue-review page. Update the Work Order to In Review if appropriate. Approval
   and execution are owner-only actions. Do not call Composio's GitHub write tools
   directly or look for an alternate path around the gate.
7. If changes are requested, submit a revision file containing `revision` (the
   current revision), `title` and `body`, using
   `python3 examples/integrations/composio-github-issues/scripts/trustgraph_actions.py revise ACTION_ID --file revision.json`.
   The revised action needs a new approval. Inspect status to see the decision.
8. Report success only when status has a published operation with a receipt.
   A simulated receipt means preview only. A live receipt contains the GitHub
   issue URL. If the result is unknown, stop and ask the operator to inspect
   GitHub/Composio; do not submit a new key to force a retry.

The gateway fixes the repository, connected account and tool slug. It binds a
Connect MCP input schema hash or a project API dated tool version, depending on
the configured Composio credential. Connect does not pin provider implementation versions.
The agent cannot select a different tool or destination. This skill guides the
workflow; the gateway enforces approval and controls the publishing credential.

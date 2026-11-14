# Live issue demo: grant, review, revoke, restore

Use GitHub issues for this demo. It reuses the verified Composio connection and
requires no code generation, branch creation or PR adapter.

This demonstrates **TrustGraph publication permission and exact human review**.
The permission switch controls this gateway, not the underlying GitHub OAuth
connection or repository membership. It does not demonstrate captured HR source
lineage. A direct GitHub credential remains an independent path outside this gate.

## Open the two screens

- 8090: the TrustGraph Demo project's agent chat. Ask for an issue proposal only.
- TrustGraph: open the private live `operator.html` printed at startup to sign in.
  The prepared live review screen runs at `http://127.0.0.1:18084`.

For a new draft, use the [8090 prompt](../examples/8090/issue-demo-prompt.md).
Copy its JSON or fenced JSON block into **Import an issue proposal from 8090**
and select **Import for review**. This operator-mediated handoff is explicit;
there is no claim that native 8090 called the gateway directly. Importing creates
no GitHub issue and records no approval.

The prepared [native 8090 draft conversation](https://factory.8090.ai/project/6b23e5ca-1d03-46a7-93dd-6a3608432db7/agents/bf1869b5-51d0-4d54-963f-b8ef8a46e312)
contains the actual agent response. Its exact final JSON is also saved as the
[fallback proposal](../examples/8090/live-demo-issue.json). It has already been
staged locally for review; importing the same unchanged JSON is idempotent.

## Rehearse these states

| Step | Action | Expected result |
| --- | --- | --- |
| 1 | Start with permission revoked; select **Try blocked publication** | Server rejects the attempt before calling Composio |
| 2 | Select **Grant permission** | Creation remains disabled; human review is still needed |
| 3 | Inspect the title, body and destination; select **Approve exact issue** | **Create GitHub issue** becomes available |
| 4 | Select **Revoke permission** before creating | Creation is blocked; the earlier approval becomes stale |
| 5 | Restore permission | **Previous approval is stale**; restoring permission does not reuse it |
| 6 | Review again, approve, then select **Create GitHub issue** | One live issue is created; the receipt links to GitHub |

Say: “The agent can propose work. Permission controls whether this gateway may
publish. Review binds the exact issue to its destination. Revoking permission
blocks publication, and restoring it does not resurrect an old approval.”

Do steps 1–5 without external writes. Step 6 is the real GitHub action. A live
receipt and matching GitHub issue are the completion evidence. An uncertain
response is never automatically retried; inspect status before further action.
Revoking permission after creation does not delete an existing issue.
Permission changes and dispatch are serialized; revoke permission before
starting creation, rather than expecting revocation to cancel an in-flight call.

The live rehearsal created [pilot issue #2](https://github.com/skishore23/trustgraph-agent-workbench/issues/2)
with title “Demo: accept zero-value payment authorizations.” The native 8090
proposal is a separate, unapproved presentation item. Its creation can be shown
live after the grant/revoke sequence; the pilot receipt remains available as
evidence that the Composio write path already succeeded.

## Restart if needed

Stop only the issue gateway process, then run from the checkout:

```sh
cargo build --locked -p trustgraph-cli
./target/debug/trustgraph actions --config trustgraph-actions-live.local.json \
  --port 18084 --agent-port 18085
```

Reopen `.trustgraph-live/issue-actions-live/operator.html` after a restart or in a
new browser. Permission changes, approval bindings and receipts survive restart.
Live and preview sessions use separate cookies. Keep the Composio key private
in the gateway; the 8090 draft needs no publishing credential.

For a rehearsal that never calls GitHub, use a separate preview state directory
and ports. The original company story at port 9081 remains the demonstration
of source permissions, restricted HR data and fresh generation.

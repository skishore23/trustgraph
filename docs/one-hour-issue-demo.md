# Guided issue walkthrough

Open the live UI at **http://127.0.0.1:18084** in your signed-in browser.
Choose a chapter on the left and select **Start this walkthrough**. The UI loads
its saved native 8090 draft, provides the next action, and updates the flow,
checks, activity, and presenter notes. There is no JSON import or terminal step
in the presentation.

## Three chapters

1. **Permission comes first.** Check the blocked gate, grant publication
   permission, inspect and approve the exact issue, verify the gate, then create
   the GitHub issue if desired.
2. **Change your mind.** Approve, revoke permission, check the block, restore
   permission, and review again. Restoring permission cannot revive old approval.
3. **Change the work.** Approve, edit the issue in the UI, check the stale review,
   and approve the new version. The editor includes an example requirement.

Only **Create GitHub issue** performs a live external write. Gate checks never
call Composio. The UI distinguishes processing, acknowledged publication,
preview receipts, and uncertain outcomes. Unknown outcomes are never resent
implicitly. **Start fresh** prepares a new local proposal; it does not delete an
existing issue or receipt.

The sidebar and publication flow link directly to 8090, Composio and GitHub.
The verified-result card opens [live pilot issue #2](https://github.com/skishore23/trustgraph-agent-workbench/issues/2).
**Presenter notes** can be hidden. **Inspect evidence** shows the exact binding,
review history and receipt without putting technical details in the main story.

## Scope and verification

This is the gateway's publication permission, separate from GitHub OAuth or
repository membership. It reuses the [saved native 8090 draft](https://factory.8090.ai/project/6b23e5ca-1d03-46a7-93dd-6a3608432db7/agents/bf1869b5-51d0-4d54-963f-b8ef8a46e312),
not a new autonomous agent run. Earlier native-agent source context is not
captured. The original company story at port 9081 demonstrates source lineage.
Revoke before dispatch; revocation does not cancel an in-flight call or delete
an existing issue.

All three chapters are tested through the HTTP server in preview mode, including
reapproval, human edits, receipt persistence after restart, and duplicate-send
protection. The live Composio path produced issue #2 with matching approved bytes.

For operator recovery only, the private launch file is
`.trustgraph-live/issue-actions-live/operator.html`. Runtime state and credentials
remain private and ignored by Git. The demo gateway runs separately from the
chat so an interrupted conversation does not stop it.

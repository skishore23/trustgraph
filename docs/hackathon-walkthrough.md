# One agent, two departments

The business case is a payment-reconciliation change at a bank. Engineering
needs an implementation. Fraud & Risk has useful confidential information that
Engineering cannot access. One agent can assist both; its broad access must not
become everyone's access.

## Before presenting

Start with `cargo run --locked -p trustgraph-cli -- start`. Open the private
operator launch file printed at startup, then `http://127.0.0.1:8080/`.
Run `python3 scripts/bank-policy-rehearsal.py` for the 16-case Rust/Hydra check.
Keep Risk investigation access set to Risk only, passing tests required and a
30-minute approval. Do not erase history to reset. The private target's main
branch remains unchanged; each approved release uses a new branch/draft PR.

The checked-in bank documents are controlled test material. State this once;
the model, executed tests, graph database and publisher are live.

## Five-minute walkthrough

| Step | Screen/action | Business point |
| --- | --- | --- |
| 1 | Workspace: show Engineering requirements and Risk investigation. Run the coding agent with Risk included. | The agent is legitimately allowed to read both. |
| 2 | Open the resulting blocked work. Inspect the actual patch and passed test output. | A useful, passing patch may still be forbidden to share. Quote detection does not establish permission. |
| 3 | Show `Risk investigation -> captured generation -> proposed patch -> Engineering`. | Hydra stores the connected evidence; Rust detects the recipient/source conflict. Approval cannot grant source access. |
| 4 | Click **Generate from approved inputs**. Inspect the new manifest: investigation absent, old patch absent. | Fresh isolated generation repairs the path. The approved spec is authorized by Risk; equivalent behavior is not claimed. |
| 5 | Approve the fresh patch, then **Revise patch after approval**. | Actual bytes change and tests rerun. The old approval no longer matches. |
| 6 | Approve again, then **Publish draft PR**. Open the real receipt link. | The gate checks before both branch creation and PR creation. The receipt is stored in HydraDB. |

The existing qualified run and [draft PR #1](https://github.com/skishore23/trustgraph-agent-workbench/pull/1)
are fallbacks if live model latency is high. They are historical real results,
not newly executed results. Never press publish repeatedly after an uncertain
outcome; inspect the journal/provider receipt.

## Policy experiments

Open **Policy**. Explain that editable parameters determine source readers,
required tests, file scope and approval lifetime. Source inheritance, missing
lineage blocking and exact approval binding are engine guarantees.

Click **Test changes**. In **Simulator**, choose **Risk -> Engineering**, then
**Compare this case**. Both policies block. Check Engineering access in the
candidate policy and compare again: the candidate allows sharing because it
explicitly grants source access. The active policy remains Risk-only. This is
not a repair recommendation; it demonstrates the consequence of a source-owner
policy change.

Click **Run all scenarios**. The table compares the same policies across Risk
sharing, approved inputs, Risk audience, patch changes, changed audience,
expired approval, missing lineage and failed tests. A source grant does not
repair missing lineage or stale approval. Try a shorter approval lifetime or
remove a test requirement to explore another policy dimension. Activate nothing
unless a source owner intends that change.

Simulation has no GitHub effects. It writes candidate policy and result evidence
to HydraDB. It does not execute code or prove arbitrary English policy; the
supported schema is explicit.

## Try from Codex

Give Codex this instruction in the TrustGraph checkout:

> Run `python3 scripts/bank-policy-rehearsal.py`. Explain which policy variants
> change an outcome and why. Then submit the committed bank work order with
> `cargo run --locked -p trustgraph-cli -- run --include-risk --goal "$(cat examples/bankcase/work-order.txt)"`.
> Expect publication to be blocked by the Risk-only input. Report the work URL
> and captured manifest. Do not use GitHub credentials, approve, activate policy,
> or publish directly. Ask the operator to use the fresh-input repair in the app.

The restricted token allows agent intake and simulation, not approval or
publication. For 8090, use the same HTTP endpoint and exact admitted work order;
the broker owns model input capture. The older MCP fixture bridge is separate.

## Closing line

“The agent can work across departments without everything it learns becoming
available to every department.”

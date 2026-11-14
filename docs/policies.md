# Small explicit policies

TrustGraph accepts a closed YAML schema, not arbitrary English rules or executable expressions. Start with [bank-policy.yaml](../examples/bank-policy.yaml); [bank-policy-short-review.yaml](../examples/bank-policy-short-review.yaml) is a second version with a five-minute approval window.

The policy declares groups, readers of each known source, actual destination audiences, eligible human reviewers and an approval lifetime of 60–86400 seconds. Every known source, including `task-prompt`, and every configured destination must be covered. Unknown principals, groups, sources, destinations, fields or rules are errors. All three rules are mandatory:

- `derived_artifact_inherits_input_restrictions`
- `approval_binds_exact_patch_and_destination`
- `missing_lineage_blocks`

`requires_approval: false` is rejected. A policy may narrow source permissions below the source authority captured at workspace initialization; it cannot add readers beyond that ceiling. Destination audiences must equal authoritative membership: removing a real repository reader from a policy would not remove their access to the repository. Changing actual membership requires a qualified source/destination integration, which this synthetic fixture does not provide.

The agent belongs to both Engineering and Risk working groups. That allows it to read both input scopes; it does not add Engineering to Risk's source readers. Maya and Sam are Engineering. Arun owns Risk. Maya can review shareable patches but cannot read the Risk investigation. Arun is the bank demo's policy administrator and has access to all four sources. Policy readers/proposers also need access to every source, preventing the policy channel from carrying confidential agent output to an unauthorized reviewer. Stored YAML is reserialized from its parsed specification; comments and aliases are not retained as arbitrary text. Policy names and other metadata are untrusted input, never executable authority.

This preview requires the policy administrator to retain the source access needed to inspect candidate policies. Production administration, identity and scoped policy-review workflows remain deployment work; fixed preview identities are not a substitute.

## Create and test versions

Run the app and open Policy studio as Arun:

```sh
cargo run --locked -p trustgraph-cli -- serve --scenario bank
```

Edit the YAML, give changed content a new `id`, validate, then Save version. A saved package is immutable. Its digest covers the parsed policy semantics. Saving does not activate it. Run the five cases and inspect the decision evidence:

| Case | Expected observation in this bank fixture |
| --- | --- |
| Allowed | Engineering-readable inputs permit preparation, with human approval still required |
| Restricted source | The Risk investigation prevents delivery to Engineering |
| Fresh-context repair | A new plan drops inaccessible inputs and recompiles |
| Stale approval | Both changed patch and changed destination fail to match the prior approval |
| Unknown provenance | An absent transformation manifest blocks verification |

The suite uses the actual Rust compiler/runtime/verifier on an isolated snapshot with local generation. It does not call an external model, push code or open a PR. A failed case means the configured policy did not exhibit that expected baseline scenario; it is not a proof that every failing policy is unsafe. Passing cases are bounded examples, not a proof about all agents or code.

For many files, use the CLI:

```sh
target/debug/trustgraph policy-check examples/bank-policy.yaml
target/debug/trustgraph policy-check examples/bank-policy-short-review.yaml
```

The command emits JSON and exits nonzero for invalid policies or failed cases. It evaluates the declared bank fixture, not a live 8090 project. A coding agent can call `trustgraph_policies`, `trustgraph_policy_propose` with `{"yaml":"..."}`, and `trustgraph_policy_test` with `{"id":"..."}`. There is no MCP activation tool.

## Activate deliberately

Only Arun can activate a bank candidate in this preview. The endpoint requires the expected active revision; stale concurrent activation fails. A single journal event records the activating principal and applies all source ACL pointers, destination revisions and the policy revision/configuration digest together. Changed policy, ACL or audience context invalidates old approvals even when the patch's text has not changed. Replay recompiles and validates the snapshot against the original ceilings. Legacy runtime ACL/audience mutations and replay also enforce the corresponding ceilings.

The local JSONL journal remains the operation authority. Hydra reconstruction currently validates historical publication evidence; it is not a live permission feed or the policy registry's authority.

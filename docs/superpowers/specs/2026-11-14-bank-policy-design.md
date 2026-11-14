# Small policy layer and bank software-factory story

The user selected the bank Engineering/Risk story and a deliberately small explicit policy language. MCP is already implemented, contrary to the illustrative pasted description. The user authorized checking the project into skishore23/trustgraph; service credentials and a dedicated publication target will follow. Do not invent live qualification.

## Policy boundary

Versioned YAML packages declare groups, source readers, destination audiences, reviewer eligibility and approval lifetime. Only three supported rules exist: derived_artifact_inherits_input_restrictions, approval_binds_exact_patch_and_destination, missing_lineage_blocks. All are mandatory; unknown fields/rules and requires_approval:false are rejected. This is configuration of the deterministic evaluator, not English-contract compilation.

Each saved package is immutable and includes its digest. Agents may propose and test packages; only the local preview policy administrator Arun (Risk owner) may activate one with an expected active revision. This is explicit demo authority, not production identity. Activation is one journal event applying source ACL revisions, destination membership revisions and the policy revision/digest together. Existing approvals become stale. A package may restrict but cannot exceed the immutable source ceilings and exact authoritative destination membership recorded at workspace initialization. Destination recipients must match authoritative membership exactly; dropping a real reader would be unsafe. It cannot invent repository membership or declassify a restricted source.

## Bank scenario

A separate bank workspace preserves existing consulting data. Maya and Sam are Engineering, Arun is Risk, and the agent belongs to both working scopes. Engineering requirements and recorded instructions are available to both teams; a synthetic Risk investigation is available only to Risk/agent. Engineering and Risk have separate configured simulated destinations. Metadata and explanations remain caller-filtered. The default simulated client flow is replaced by bank-specific source/recipient labels, with the same actual compiler and review boundary. Generation is still clearly labeled fixture generation until a real isolated model broker is qualified.

## Tests and UI

Policy UI: saved versions, small YAML editor, validation, five-case test report, explicit activate button with current revision. It shows supported mandatory rules and source authority ceilings. MCP adds list/propose/test operations, never activation or publication. A reproducible five-case suite uses the actual Rust compiler/runtime on an isolated in-memory snapshot: allowed, restricted source, fresh-context repair, stale patch/destination approval, missing provenance. Reports include observations and expected outcomes, not a universal security proof.

## Real adapters

Extend the existing Hydra client with namespace configuration and validated readback reconstruction of a stored bundle; retain local journal as operation authority. Missing endpoint/token means live qualification remains pending. Composio must gate both branch content and PR creation after approval, use an explicit connected account/repository, disable implicit mutation retries and reconcile uncertain outcomes by reads. Do not ship an adapter that pushes a branch before approval or accepts a caller's unsupported source declaration. If the required remote contract is not yet qualified, keep it disabled with a concrete readiness report rather than claiming the full story works.

## Delivery

Run existing regressions plus policy configuration/authorization/replay tests, verify the browser, check in a private new repository at the requested location, and document the exact implemented versus pending boundaries. No real bank data or account secrets enter the repository.

Policy readers/proposers must be authorized for every source. Canonical serialization drops user YAML comments and normalizes aliases before storage/display. Free-text metadata is untrusted, and is never used as an authorization rule. Legacy runtime ACL/audience mutation and replay also enforce the root reader ceilings.

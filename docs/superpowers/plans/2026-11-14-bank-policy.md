# Bank policy layer implementation plan

> Use superpowers:executing-plans in this session and a read-only independent final review.

**Goal:** User-authored bounded policies exercised by the bank scenario and an MCP coding-agent test loop.

**Architecture:** YAML -> validated immutable package -> atomic current evidence update. Source/destination ceilings remain rooted in journal genesis; the same compiler and verifier evaluate candidate snapshots.

**Spec:** docs/superpowers/specs/2026-11-14-bank-policy-design.md

- [x] Add red tests for unsupported rules, permission widening, atomic activation, stale approvals, agent inability to activate and deterministic five-case suite.
- [x] Add policy model/parser, runtime registry/journal events and candidate evaluator. Preserve old policy hashes with omitted optional configuration digest.
- [x] Add bank fixture and selectable server scenario with separate data directory; use same source/recipient simulator with actor-scoped bank metadata.
- [x] Add policy HTTP/MCP list, propose and test; human-only activation. Add minimal policy editor/test UI.
- [x] Extend Hydra namespace and validated bundle reconstruction; test transport/readback failure paths. Record live gate status honestly.
- [x] Prepare Composio contract/configuration and qualification boundary without pre-approval branch writes or live publication before account/target details.
- [x] Run checks, browser verification and independent review. Create/push requested private repository with secrets/data ignored. Report the remaining live gates.


## Execution record

Implemented policy parsing, canonical immutable storage, source/destination authority, atomic human activation, five-case reports with actual verifier evidence, HTTP/MCP authoring/testing, bank fixture, scope-filtered simulator and readable patch preview. Added two example policy versions and policy-check CLI. Hydra gained namespace configuration and validated historical bundle reconstruction with complete/missing-marker HTTP contract tests. Composio remains an explicit next-stage contract, not an enabled adapter.

Ruling after security review: Arun is the bank policy administrator because he can read all source scopes; Maya can review Engineering-safe patches but cannot inspect Risk-scoped policy candidates. Proposers/readers need all-source access, YAML comments/aliases are discarded by canonical serialization, runtime/replay ACL mutations respect ceilings, and destination declarations cannot drop actual recipients. Demo administrators must retain candidate-inspection access. Malformed-policy tests use an authorized agent and assert validation errors, not merely authorization failure.

Validation: 71 workspace tests passed, zero failed, one ignored live Hydra contract. Formatting, warnings-as-errors Clippy, JS syntax and executable build passed. Both example policies pass five cases. The browser saved/tested/activated a second candidate, confirmed revision 2 survives restart, and verified Maya sees only the repaired patch and public inputs. Live local stdio evidence is in evidence/bank-mcp-trial.json: ten tools, policy proposal/testing, concrete Risk source conflict, fresh-context repair and a real tracked review awaiting a human; no external publication. The live Hydra and Composio credentials/target were not supplied, so those gates remain open.

The user authorized private repository creation/check-in at skishore23/trustgraph. Local data, capability tokens and target build outputs are ignored. Repository publication is the final delivery step after verification.

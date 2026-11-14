# Live qualification — 14 November 2026

The original Engineering / Fraud & Risk case was exercised with a real fresh
Codex generation, hosted HydraDB governance graph, isolated Rust test runner
and authenticated GitHub publisher. Bank sources are controlled test material;
department audiences are explicitly configured.

| Check | Observed result |
| --- | --- |
| Risk-context generation | `job-7cb2cc3d823d4e54ad43404709e84af9`; actual Rust tests passed, source flow blocked |
| Attempted approval | HTTP 400; source/audience boundary denied |
| Attempted publication | HTTP 400; restricted source denied |
| Fresh repair | `job-184eef5cfd93433ba5cca498fc4d5b3b`; five inputs, investigation excluded, old patch not supplied |
| Actual patch revision | Tests rerun; evaluator returned `STALE_APPROVAL` |
| Stale publication attempt | HTTP 400; no GitHub write |
| New approval | Recorded/read back in HydraDB with exact current context |
| Actual publication | [https://github.com/skishore23/trustgraph-agent-workbench/pull/1](https://github.com/skishore23/trustgraph-agent-workbench/pull/1); `isDraft: true`, `simulated: false` |
| Exact GitHub bytes | `src/lib.rs` retrieved from PR commit and compared byte-for-byte with approved payload |
| Main branch | Unchanged; draft PR only, no merge |
| Policy rehearsal | 16 supported policy/scenario combinations passed; active policy and jobs unchanged |

GitHub commit: `afdaeb442f0ee487157b0a3f7795b524bcc04f5e`.
Publication receipt graph record: `publication-receipt-bc3a652d1ec3c34fa842cb95da1759bfe60160f964239400e6f064b508ddec1f`.
Governance store: HydraDB database `trustgraph`, BYOG collection
`trustgraph-governance`; department evidence namespace `bank-departments`.

The simulator includes source-reader grants, Engineering/Risk audiences, exact
patch/audience/policy approval binding, expiry, missing lineage, required tests
and file scope. These are structured supported conditions, not arbitrary English
rules. Its candidate policies and case evidence are persisted in HydraDB; no
GitHub writes occur during simulation.

The workspace test suite passes 85 tests with one ignored standalone-Hydra
fixture contract. `cargo clippy --workspace --all-targets -- -D warnings` and
formatting checks pass. The hosted-Hydra flow above was qualified separately.
The restricted agent credential cannot approve, activate policy or publish.
Agent work orders must be predefined or admitted by the operator; new input text
cannot be declared public by an agent caller.

The clean URL normalization was observed in the browser: old
`?theme=ascii#/demo` became `/`. The operator reported completing sign-in in their
browser. Automated authenticated visual checking was not completed because
browser control was stopped; UI behavior still needs that visual pass. HTTP
policy, source-flow, actual revision and publication paths were exercised.

Real publication uses authenticated GitHub API calls via `gh`. Composio and a
live-workspace MCP transport are not qualified. The existing MCP fixture bridge
is a separate offline path. Controls apply to the brokered agent and configured
permissions, not an external agent holding independent publishing credentials
or unseen context. No code-correctness or absolute secret-free guarantee is made.

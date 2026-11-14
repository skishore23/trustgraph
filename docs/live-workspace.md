# Live policy workspace

Open `http://127.0.0.1:9080/`. The three views are **Policy**, **Simulator** and
**Agent**. A policy is a set of business rules and settings. Allowed files,
maximum changed files, required test results and approval lifetime are editable.
The [hackathon walkthrough](hackathon-walkthrough.md) provides a five-minute
story, exact screen actions, a 14-case rehearsal suite and a Codex trial prompt.

Source restrictions, captured lineage and exact approval binding are always
enforced by the Rust engine; they are not repeated in each policy.

The currently configured workbench is a dedicated private repository:
https://github.com/skishore23/trustgraph-agent-workbench. Its `valid_amount`
function rejects zero-value authorizations. The work order asks Codex to fix that
bug and add a regression test. The other inputs are committed TrustGraph code and
documentation. These are real repositories and actual model/test executions;
there is no confidential bank investigation in this live workspace.

## Start and submit work

Requirements: Rust, authenticated `hydradb`, `codex` and `gh` CLIs; macOS
`sandbox-exec` for the isolated Rust runner. Credentials are read locally. Do not
commit local configuration, tokens, model transcripts or runtime state.

Create `trustgraph.local.json` with absolute paths:

```json
{
  "directory": "/absolute/TrustGraph/.trustgraph-live/state",
  "context_repository": "/absolute/TrustGraph",
  "context_files": ["README.md", "crates/trustgraph-core/src/verifier.rs"],
  "target_repository": "/absolute/TrustGraph/.trustgraph-live/workbench",
  "github_repository": "skishore23/trustgraph-agent-workbench"
}
```

```sh
cargo run --locked -p trustgraph-cli -- start --port 9080
```

On first use, open `.trustgraph-live/state/operator.html` to sign in. This private
local launch file exchanges an operator token for an HttpOnly session, then removes
the token from the URL. Merely opening the app grants no review permissions.

Then use the Agent form or a second terminal:

```sh
cargo run --locked -p trustgraph-cli -- run \
  --url http://127.0.0.1:9080 \
  --goal "Fix valid_amount to accept zero, reject negatives, and add a zero regression test"
```

The CLI uses `.trustgraph-live/state/agent-token`, streams actual activity and
stops when review is required. The agent token can submit/read work and compare candidate policies in the
simulator. It cannot approve, activate policy or publish. Another agent or 8090 can call the same HTTP intake:
`POST /api/work` with `Authorization: Bearer <agent token>` and a JSON `goal`.
This delegates generation to the controlled fresh Codex broker. It does not
accept a caller's claim that an arbitrary patch has trustworthy provenance. The
older MCP bridge targets the synthetic workspace and is not a live-workspace
MCP integration.

## Walkthrough

1. **Policy:** allow only `src/lib.rs`, one changed file, passing tests and a
   30-minute approval. Test an unsaved candidate or save a new active version.
2. **Simulator:** edit a candidate policy or import supported JSON. Compare the
   same proposed change against active and candidate policies. Presets cover code
   + docs, failed tests, changed patches/policies, missing inputs and expiry.
   Results separate information flow, business rules and approval, with concrete
   failure paths and zero publication effects. Nothing is activated. Custom safe
   relative paths are supported here; the live runner retains its two-file scope.
3. **Agent:** submit the work order. See the actual code before/after, captured
   plan/input manifest, provider metadata, isolated test output and live activity.
4. Review the exact patch, then **Approve exact change**. **Run publication gate**
   checks live Hydra evidence and current Rust authorization. The default action
   is simulated; no branch or PR is created.

Policy changes invalidate the authorization context. Existing work must be
regenerated under the active business policy. An approval of an older patch is
not permission for a changed patch. Human approval cannot override missing
lineage or a source-access violation.

## HydraDB is on the decision path

The hosted adapter uses the authenticated HydraDB BYOG API, database selected by
`hydradb`, collection `trustgraph-governance`. The BYOG graph supplies the actual
source bytes to the generation broker. The current source snapshot comprises two
committed TrustGraph files and three workbench files, not an entire repository. Content fingerprints and exact incoming
`USED_AS_INPUT` relationships are read back before review and effects.

The five captured source documents were also registered explicitly in HydraDB
**Knowledge**, using the same source IDs. Knowledge ingestion is optional and is
not performed automatically on startup. In the Hydra dashboard select database
`trustgraph`, collection `trustgraph-governance`. Knowledge indexing is asynchronous;
`queued` does not mean searchable. The BYOG governance graph and the Knowledge
source list are separate namespaces, and the graph record count is not a Knowledge
source count. The app shows registration status separately for each captured file.

Sources, prompts, plans, policies, generated artifacts, manifests, test receipts,
approvals, simulator results and gate outcomes have content-addressed records.
After approval the adapter also writes and reads back the normalized evidence
projection: principals, source ACLs, artifacts, transformation input occurrences,
audiences, policy, exact approval context and bundle, joined by typed relations.
Each authorization snapshot has its own namespace so later snapshots do not
change an earlier snapshot's relationship set. Historical replay validates the
snapshot; the Rust coordinator separately checks current authorization.

HydraDB being unavailable or evidence/relationships failing validation stops the
path. The local journal persists reservations and current coordinator state;
it is not a substitute for the live graph checks. The graph database does not
execute the permission rules itself.

## Real execution and current scope

A fresh Codex turn has external tools disabled, no resumed session, no repository
instructions and only the broker-supplied input bytes. The broker rejects tool
activity or failed/incomplete turns. It captures the prompt, schema, input
manifest and provider result. This checks the modeled input boundary; it is not
a proof about the model's training data or the semantic correctness of code.

The host compiles and executes the returned `src/lib.rs` tests with `rustc --test`
in a macOS sandbox. No network or home-directory file contents are allowed.
Compilation, tests and model execution have time and output bounds. This narrow
runner intentionally supports no dependencies or build scripts. The initial
policy supports `src/lib.rs` and `README.md`, rather than arbitrary repositories.

The live audience is the sole owner of a private personal GitHub repository.
Local operator authentication is separate from agent authentication. This is not
organization IAM, automatic department ACL import or multi-user approval.
The bank's Engineering/Risk story remains a separate synthetic fixture.

An optional server-owned GitHub provider exists behind the explicit real-draft-PR
control. It rechecks the private repository's readers, exact payload and base,
records an attempt before writes and never blindly retries an uncertain effect.
The default walkthrough does not invoke it. Composio is not configured or
live-qualified. The standalone Hydra protocol adapter and its separately ignored
contract test remain independent from this hosted integration.

## Test policies from Codex

```sh
python3 scripts/hackathon-check.py
cargo run --locked -p trustgraph-cli -- policy-simulate \
  --case examples/payment-change.json \
  --policy examples/payment-policy-expanded.json \
  --expect allowed
```

The CLI uses the restricted agent token and prints active/candidate outcomes.
`--json` returns full evidence; `--output FILE` writes it; `--expect` verifies a
verdict for scripts. Supported policy fields are `name`, `allowed_paths`,
`max_changed_files`, `require_tests` and `approval_minutes`. Approval assumptions
are `valid`, `missing`, `stale`, `previous_policy` and `expired`. This is a typed
policy experiment, not an arbitrary English policy interpreter or code runner.

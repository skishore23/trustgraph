# Live bank workspace

The app has three pages: **Workspace** runs/reviews the agent, **Policy** sets
access and execution conditions, **Simulator** compares policy changes across
hypothetical cases. All use the same Rust evaluator. No page is named Demo and
appearance is fixed ASCII. Routes are `/`, `/policy`, `/simulator` and `/agent/{id}`.

## Setup

Requirements: authenticated `hydradb`, `codex` and `gh` CLIs, Rust, macOS
`sandbox-exec`. Credentials remain local. Never commit tokens, transcripts or
runtime state. Use a dedicated private GitHub repository with its sole personal
owner; the publisher checks privacy/collaborators before sending. It will not
publish into an arbitrary shared or public repository.

The current target is `skishore23/trustgraph-agent-workbench`. Its `main` remains
unchanged for repeatable runs. Initialize another target with the same small
Rust fixture if needed, then set it explicitly in the local config. Existing
state namespaces must not be repurposed for different audiences.

```json
{
  "directory": "/absolute/TrustGraph/.trustgraph-live/bank",
  "context_repository": "/absolute/TrustGraph",
  "context_files": [
    "examples/bankcase/engineering-requirements.md",
    "examples/bankcase/risk-investigation.md",
    "examples/bankcase/approved-specification.md"
  ],
  "target_repository": "/absolute/TrustGraph/.trustgraph-live/workbench",
  "github_repository": "skishore23/trustgraph-agent-workbench",
  "departments": true
}
```

Save as ignored `trustgraph.local.json`. Source imports read committed Git bytes;
commit controlled source material before starting. Only one server may own a
state directory. `departments: true` creates the `bank-departments` evidence
namespace; earlier personal-workbench histories stay in their original directory.

```sh
cargo run --locked -p trustgraph-cli -- start
```

Open the printed private `operator.html` file once. It establishes an HttpOnly,
SameSite local session and clears the launch fragment. The browser root itself
does not grant operator authority. Then use `http://127.0.0.1:8080/`.

## Agent intake

```sh
cargo run --locked -p trustgraph-cli -- run --include-risk \
  --goal "$(cat examples/bankcase/work-order.txt)"
```

The CLI resolves the restricted agent token from the configured state directory.
It streams actual generation/test stages; the Risk-context case ends blocked.
Omit `--include-risk` for approved sources only, or use the app's fresh repair.
8090 can call `POST /api/work` with `{ "goal": "<admitted work order>",
"include_risk": true }` and the restricted Bearer token. Generation is performed
by TrustGraph's controlled fresh Codex broker. Caller-provided patches or claims
of complete lineage are not accepted.

An agent may trigger the committed `examples/bankcase/work-order.txt` task or an
exact task previously admitted by the operator. It cannot invent a new public
input by putting previously restricted material in a freeform goal. New human
work orders can be submitted in the operator UI. Repairs retain that admitted
goal and start a fresh model turn with the investigation and old patch excluded.

The live integration is HTTP/CLI. The existing fixture MCP bridge targets the
older offline app and must not be presented as this live transport. A general
external agent with unseen context/direct GitHub credentials remains outside
this enforcement boundary.

## Evidence and publication

HydraDB `trustgraph` / BYOG `trustgraph-governance` stores source bytes, source
permissions, goals, plans, prompts, manifests, generated artifacts, checks,
approvals, normalized authorization graphs and publication receipts. Before
approval/publication the runtime reads back exact content, parent sets and
current permission evidence. Knowledge indexing is optional browsing support;
its status is separate from required governance graph writes/readbacks.

Rust checks inheritance, captured lineage and exact context. GitHub's real
adapter revalidates the configured repository readers, unchanged base commit,
checked patch digest and exact file bytes. It journals intent before mutation,
creates a branch and draft PR and records an actual receipt in HydraDB. An
uncertain outcome is never blindly resent. No simulated publisher is exposed as
the live app's release action. `/gate` delegates to this same real publisher.

The operator may approve, revise the actual patch (tests rerun and approval
becomes stale), approve again, then **Publish draft PR**. Publication requires
both current authorization and Hydra-recorded approval. Review cannot bypass
source restrictions. No PR is merged automatically.

## Simulator

Supported fields: `name`, exact `allowed_paths`, `max_changed_files`,
`require_tests`, `approval_minutes`, and
`departments.investigation_readers` (`risk` always; `engineering` optional).
A source grant is an explicit source-owner policy change, not a human review
exception. Unknown fields, unsupported readers and arbitrary English rules fail
validation. The simulator accepts safe custom file paths; the live runner is
currently limited to `src/lib.rs` and `README.md`.

Scenario fields include Risk input, Engineering/Risk audience, test outcome,
manifest completeness, approval age and missing/stale/expired approval. Active
and candidate decisions are compared for the same hypothetical inputs. **Run all
scenarios** evaluates eight cases in the UI. The CLI rehearsal evaluates sixteen,
including additional policy variants:

```sh
python3 scripts/bank-policy-rehearsal.py
```

Simulation makes no GitHub writes, activates no policy and executes no proposed
code. Its inputs, candidate policy and results are persisted in HydraDB. Actual
agent work executes its own real tests and still requires current authorization.

## Boundaries

Bank documents are controlled test material. Department permissions are explicit
configured identities, not imported enterprise IAM. The local owner is trusted
to admit inputs and is authorized to read Risk material. This is one local
workspace, without multitenancy or enterprise SSO. The model runner disables
external tools, uses a fresh turn and cannot publish. Tests run with filesystem
and network restrictions. These controls check modeled lineage/access, not code
correctness or an absolute absence of secrets. Composio is not configured; real
publication currently uses `gh`-authenticated GitHub API calls.

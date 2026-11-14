# Plan language, compilation and execution

`examples/launch-plan.json` is an executable-language proposal for the synthetic
Northstar workspace. `compile-plan` evaluates it against that fixture snapshot;
the API evaluates proposals against the active workspace's current evidence.

A plan has an immutable ID, a descriptive goal of up to 512 UTF-8 bytes and 4–32
ordered steps. References point backward, preventing cycles and forward/dangling
inputs. Exactly one Approve is penultimate and exactly one Publish is final.
Publish references that approval and the same generated artifact step/audience.
Multiple approval/resume points are not part of this language version.

| Operation | Contract |
| --- | --- |
| Read | Resolve an exact source revision; validate its references and current caller/source access |
| Generate | 1–64 typed input occurrences, including a direct recorded task-prompt input; a fresh `local_fixture` context; output readership is the meet of all inputs |
| Approve | A runtime obligation after generation, referring to actual output and a known audience; compilation grants no human authority |
| Publish | Exact agreement with the approval obligation, complete lineage and finite recipient containment; current evidence and actual approval are checked at dispatch |

The compiler returns source-to-output reader sets, effects, obligations and the
first conflict. A recipient outside the derived set is paired with one source
that excludes them and its dependency path. Private metadata is returned only to
authorized plan readers, independent of where a syntax error occurs.

Fixed repairs produce immutable alternative plans and run the same compiler:
use a configured permitted audience, or replace the final generation's inputs
with permitted source reads and prune discarded branches. The latter starts a
new context, not an edit of a tainted draft. Information and wording may differ.
Neither repair updates policy or ACLs. Candidate identities include their actual
repaired operation body so later changed alternatives do not overwrite history.

## Runs and approval

The run reservation pins plan/snapshot digests, exact source bindings and the
identities of anticipated generated artifacts before any generation. Local
generation is idempotent under those identities. A partial run can resume only
under the same snapshot. Its output cannot be approved or published through a
lower-level route while the run is still preparing.

Preparation creates an ordinary human review, then records the complete run
bindings. A long goal receives a UTF-8-safe bounded review title; the full goal
stays in metadata. The generated content comes from the fixed launch template
and declared inputs, not from natural-language goal interpretation.

At approval and publication, the runtime checks actual transformation inputs,
provider contract, source versions, selected output and intended audience against
the prepared plan. Edits or substitutions require a new run. Current source ACLs,
policy, signer authority and exact-content approval are still enforced by the
existing verifier. Request linkage is durable before the publisher is called.

## Interfaces

- `POST /api/plans`: authenticated agent proposal; body is the typed plan JSON.
- `GET /api/plans` and `GET /api/plans/{id}`: authorized current analyses and runs.
- `POST /api/plans/{id}/repair`: `{"kind":"fresh_context"}` or `narrow_audience`.
- `POST /api/plans/{id}/prepare`: current `plan_digest`, `snapshot_digest` and an
  idempotency `key`. Returns a run and its human review request identity.
- `POST /api/verification`: `{"max_depth":6,"broken_rule":false}`. The optional
  broken rule affects only the finite model, never the real execution gate.

Reviews/releases retain the existing API documented in `agent-intake.md`.
The [TrustGraph MCP interface](mcp.md) exposes the same plan operations to agents.
Real model providers, real Hydra plan reads and Composio execution remain next stages. Category-shaped composition here is a checked schema
and explicit predicate system, not an automatic proof of semantic secrecy.

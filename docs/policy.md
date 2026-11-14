# Policy and simulation

TrustGraph separates source access from approval. A reviewer must be allowed to
access the work, and review cannot grant a department access to restricted
information. Derived artifacts retain their recorded input restrictions.

## Company policy: YAML

The guided company workspace uses the strict Rust `PolicySpec` format. A complete
example is [company-policy.yaml](../examples/company-policy.yaml).

```yaml
groups:
  hr: [nina]
  engineering: [sam]
sources:
  compensation:
    readers: [hr, generation, governance]
destinations:
  engineering:
    audience: [engineering]
    requires_approval: true
review:
  approvers: [leo, maya, nina, sam]
  max_age_seconds: 1800
```

This is an excerpt: the complete file also defines schema version, identity,
remaining groups/sources/destinations and required rules. Rust resolves group
membership into concrete principal sets. Unknown names, unsupported schema or
attempts to exceed configured source/destination authority are rejected.

In the UI, edit a proposed, test it against the current policy and only then
activate it. Proposed policies can change source readers and approval lifetime. Changes
to a source boundary require source-owner authority; approval is not an exception
to that boundary.

## Live coding workspace: JSON

The repository runner exposes a smaller `WorkPolicy` contract. This is **not** a
YAML upload endpoint. Its fields reflect the currently supported code executor:

```json
{
  "name": "Reviewed Rust changes",
  "allowed_paths": ["src/lib.rs"],
  "max_changed_files": 1,
  "require_tests": true,
  "approval_minutes": 30,
  "departments": { "investigation_readers": ["risk"] }
}
```

`allowed_paths` contains exact relative file paths. `max_changed_files` is 1–10;
`approval_minutes` is 1–1440. In department mode, investigation readers must
include `risk`; `engineering` is the only optional additional audience. Unknown
fields and arbitrary English rules fail validation. Omitting `departments` is
supported for the personal-workbench mode. Live execution currently supports
`src/lib.rs` and `README.md`; simulation can test additional safe paths.

## Test before activation

Use the guided simulator without services, or the live Simulator/MCP/CLI with
a configured Hydra workspace. Both use the Rust verifier; the live wrapper also
checks file scope and actual-run test requirements.

```sh
./target/debug/trustgraph policy-simulate \
  --case examples/payment-change.json \
  --policy examples/payment-policy.json --expect blocked
```

The example assumes approval age 20 minutes. The proposed shortens its lifetime
to 15 minutes, so the proposed blocks. The active result depends on the workspace’s
current policy; restore its 30-minute lifetime for the matching allowed baseline.

Live scenario fields: `paths`, `tests_passed`, `lineage_complete`, `approval`,
`approval_age_minutes`, `include_risk`, `audience`, and optional full `policy`.
Approval assumptions are `missing`, `valid`, `stale`, `previous_policy`,
`expired`, or `destination`. Audience is `engineering` or `risk`.

| Experiment | Expected effect |
| --- | --- |
| Include investigation; Engineering has no source grant | Source boundary blocks, even with assumed approval |
| Use shared sources; missing approval | Review required |
| Change exact artifact, destination or policy after approval | Prior approval does not authorize the new context |
| Approval age exceeds lifetime | Expired approval blocks |
| Missing lineage or failed required tests | Blocks |
| Change proposed source readers | Recompute inherited access; does not activate the grant |

Simulation does not generate a patch, execute code, activate policy or publish.
The live API records its assumptions and comparison in HydraDB. The guided story
fixes its clock and evidence for identical result fingerprints; live requests use
server time. See [reproducibility](architecture.md#simulation-and-reproducibility).

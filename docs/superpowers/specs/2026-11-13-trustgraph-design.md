# TrustGraph design

The supplied brief defines a gateway that carries source readership through an
agent's recorded inputs, binds a human decision to exact content and audience,
and records the result of controlled publication. The first workflow is the
synthetic Northstar launch announcement. No real company data or external posts
are required to demonstrate it.

## Approach and boundaries

Build a Rust workspace with a framework-independent decision library, a runtime
for storage and effects, a deterministic simulator, and one CLI/API executable.
The simulator invokes the same verifier and publication coordinator as the API.
Time, publisher behavior, and durable storage are explicit inputs. No simulator
branch may bypass policy. This is the implementation choice for this delivery.

A database-first build would qualify Hydra earlier but delay testing the gate.
A UI-first build would demonstrate interaction before establishing correctness.
We choose a complete local decision loop first, with an explicitly separate
Hydra qualification stage. The supplied brief already authorizes implementation;
routine reversible choices are made here without a second approval round.

## Invariants

- Immutable artifact versions hash exact UTF-8 bytes with SHA-256.
- Structured hashes use compact sorted-key JSON inside a versioned domain envelope.
- Each generated artifact has one producing transformation and an ordered broker
  manifest. Repeated input occurrences remain distinct. Missing records, cycles,
  scope mismatches, unknown recipients, and traversal limits block publication.
- Output readership is the intersection of all transitive source readership.
  Approval never expands readership. Empty provenance blocks.
- The approval binds content, artifact identity, audience and destination, action,
  policy, complete lineage, and current source ACL revisions. Signer eligibility,
  revocation, and expiry are checked again at dispatch.
- Publication holds one coordinator ownership boundary across current verification,
  durable dispatch intent, effect, and outcome. An operation key has one payload.
- A timeout after acceptance records uncertainty. Reconciliation inspects the
  original operation; it never submits a second post blindly.
- A local private journal is authoritative for the initial slice. Hydra is a
  rebuildable evidence projection until actual server contracts are qualified.
- Saved evidence can explain an earlier decision, not certify current permission
  or collector honesty. Unsigned digests detect inconsistency, not malicious
  rewriting of an entire bundle.

## Deliverables

1. Pure schema, lineage, finite-set policy, approval matching, and replay checks.
2. Small seeded scenario runner with virtual time, typed traces and fault scripts.
3. Durable local journal, idempotent simulated publisher, startup recovery, and
   evidence export. Process ownership uses a filesystem lock.
4. Loopback-only synthetic API and review interface: restricted draft, clean
   regeneration, approval, edit, stale block, reapproval, receipt.
5. Parameterized Hydra HTTP projection/client and opt-in real contract check.
   A passing HTTP fixture is not a passing Hydra integration.

## Explicit limits

The local API is a synthetic development surface. Actor identity is selected
from fixed demo credentials; enterprise identity, real model access, live
publishing, remote policy races, production secret management and multi-node
coordination are follow-up stages. The first UI is served by Rust with no Node
server. The evidence journal includes synthetic content; content-addressed blobs
are additional local storage, not an unauthenticated retrieval interface.

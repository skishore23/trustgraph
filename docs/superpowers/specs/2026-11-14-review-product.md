# TrustGraph: agent review inbox

## Product decision

The user selected an agent-first intake: existing agents submit work for review.
TrustGraph's central job is helping a human decide whether an agent's proposed
communication can leave the workspace. One installed workspace, no tenant chooser,
subscription, billing or organizational provisioning.

The initial user is a product or communications reviewer. The durable object is a
review request: a named piece of work, submitting agent, exact artifact revision,
proposed destination, evidence, feedback and release result. The primary action
is Review, followed by Approve & release or Request changes.

An embedded drafting suite would compete with the user's existing agents and
make creation the main interaction. A generic policy dashboard would center
configuration before useful work. We choose a review inbox because it matches
the user's explicit ingress choice and reuses the existing Rust gate directly.

## Interaction

Navigation: Inbox, Releases, Sources; Destinations and Review policy are small
read-only configuration views. Connect agent opens working API instructions and
clearly identifies the fixture-based local integration. Seeded requests are
labeled as samples. No invented metrics, notifications or disconnected controls.

Inbox opens with a compact request table, meaningful title, agent, destination,
status and submitted time. Filters: All, Ready for review, Needs attention,
Changes requested. Search filters actual request metadata. Review opens a
dedicated request page, not an editor or marketing hero.

Request layout: content preview and source evidence in the main column; intended
destination/recipients, current checks and decision controls in a right rail.
Approval confirmation displays the exact content and destination again. Request
changes records written feedback and revokes applicable existing approvals.
An agent revision is a new immutable artifact with an expected-current-revision
check. Historical feedback survives. A revision clears the changes-requested
state but cannot reuse approval. Permission constraints still apply.

Release history comes from real local publication operations. Uncertain outcomes
offer reconciliation of that operation; no retry/send-again button. Evidence
downloads use existing permission-filtered exports.

## Implementation choices

Retain Axum and embedded browser assets. Use a persistent sidebar, compact type,
neutral surfaces, violet accent, restrained status colors and responsive request
layout. Build with semantic HTML and small browser modules; no frontend runtime
migration is needed for this product correction. Persist submissions and review
feedback in the existing journal. Existing events remain readable.

Expose authenticated submission, revision, feedback and release endpoints. Only
agent principals submit/revise; authorized human reviewers return or release.
List/detail reads require reviewer/submitting-agent scope and access to historical
request artifacts, avoiding private feedback leaks after a safe revision.

Intake accepts only artifact IDs already recorded by the trusted broker. It does
not ingest arbitrary text with agent-declared provenance. An edit retains the
previous draft as an input; a clean regeneration uses the broker's fresh context
path. Resubmission references the new tracked artifact and never rewrites lineage.
Real external-agent capture is a later adapter contract, not implied by the preview.

Seed four real sample requests through runtime methods: a restricted company
announcement, a product-only preview, a safe roadmap update and an edited draft
with stale approval. Each badge derives from current verifier/operation state.
The simulator/publisher remains explicitly local. Sources and destinations are
real configured fixture records, not claims of connected enterprise services.

## Acceptance

Agent submission is idempotent and survives reopen. Agent cannot approve; Sales
cannot retrieve reviewer requests/private content. Feedback is durable and
prevents release until resubmission. Stale browser revisions cannot release a
newer artifact. Two identical release clicks cause one operation. Current source
checks remain authoritative. The browser can complete review, request changes,
and release workflows. Full Rust gates and existing simulations stay green.

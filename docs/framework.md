# Permission-preserving dataflow framework

## What the framework models

TrustGraph models data dependencies, not just a log of requests. A request ID
identifies an invocation; an artifact ID identifies the exact value consumed or
produced. The same artifact can be consumed by many executions.

| Concept | Meaning |
| --- | --- |
| Artifact | Exact bytes, content digest, schema identity and source or execution origin |
| Operation | Versioned declaration of named input/output ports and expected schemas |
| Execution | One operation invocation with exact input occurrences and outputs |
| Source restriction | Current permitted readers and a revision |
| Action intent | Actor, action, artifact, destination and resolved recipients |
| Decision/approval | Permission result and, where required, approval of its exact context |

The executable function belongs to the integrating application. A database row
can reference `create_quote@1`, but storing that name does not prove that the
claimed function executed. The trusted integration records its execution.

In relational terms, the graph is artifacts and executions joined through input
and output references. In graph terms, it is a bipartite artifact–execution graph.
Multi-input/multi-output operations need not be forced into a single linear chain.

## Typed means two different things

Rust domain types distinguish artifacts, operations and executions. Port schema
identities constrain which artifact may connect to which input. Schema identity
matching does not validate arbitrary payload semantics; the application owns
schema validation. Runtime permission labels are separate from these data types.
No compile-time theorem about arbitrary external code is implied.

## Information flow, action policy and approval

These are different checks:

1. **Information flow:** every recipient must be permitted by every recorded
   source on the artifact's lineage.
2. **Action policy:** the application permits this actor to perform this action.
3. **Approval:** a qualified reviewer approves the exact context if required.

The first invariant cannot be widened by the other two. An approval is not
implicit declassification. The initial framework deliberately has no generic
redaction exception or automatic “this output contains no secrets” inference.

Every output inherits the intersection of all input reader sets. Extra context,
parameters containing sensitive data and tool responses must themselves be
captured as governed inputs. An application cannot put a secret in an ungoverned
parameter and expect the graph to discover it.

## Lifecycle and enforcement

Register trusted source versions and versioned operation contracts. Before
execution, validate the named inputs and the actor/executor's access. Record the
exact outputs after execution. Before an external effect, evaluate its exact
intent against current source restrictions and any approval requirement.

The library's decisions only protect effects routed through the integration.
Keep credentials in the trusted adapter, not in an agent that can call the
external service around the gate. If rules change while a computation is in
flight, recheck before admitting its output or disclosing it. That cannot undo
information already handed to the executor before revocation.

## Storage and HydraDB

The SDK's initial graph is in process. It does not automatically persist records
into HydraDB. The existing gateway's Hydra adapter stores its legacy evidence
projection and checks it on read-back. The framework and gateway share the
reader-set restriction algebra; they are not two graph databases competing to
be authoritative.

Integrating the generic SDK with a durable store requires an explicit persistence
contract and trusted current-policy snapshots. Updating a database and sending
an external effect are not one transaction. Production adapters need idempotency,
unknown-outcome reconciliation, authentication and snapshot/freshness semantics.
The existing gateway retains its journal and publisher recovery behavior.

## Composition and category-theoretic motivation

The concrete useful law is that composing transformations must not broaden
access. Reader-set intersection is associative, commutative and idempotent;
adding an input cannot increase the allowed audience. Provenance still preserves
which port and occurrence used each artifact even though intersection ignores
ordering.

Typed port compatibility gives a disciplined way to compose operations. That is
not by itself a proof that arbitrary effectful or nondeterministic functions form
the desired category. Identity, composition and equivalence laws would need an
explicit semantics. Tests exercise the finite reader-set model; no general
formal verification or semantic noninterference claim is made.

## Migration scope

The generic SDK is an additive extraction that leaves existing gateway/demo
formats intact. Current demo policy YAML, fixed generation fixtures, the
publication-only legacy verifier and hosted MCP endpoints remain application
code. They are not advertised as the generic framework's interface.

This preserves existing users while a framework consumer can integrate without
Codex, GitHub, a web UI or Hydra. Remaining work includes a durable generic
adapter, authentication and administration surfaces, richer policy dimensions,
blob storage and operational hardening. Do not treat the in-process prototype
as a production security boundary on its own.

## SDK calls

```text
register_source(logical_id, schema, bytes, readers) -> artifact ID
register_operation(definition) -> operation ID
begin_execution(operation, named inputs, actor, parameters) -> opaque ticket
finish_execution(ticket, named output bytes, receipt) -> named artifact IDs
set_action_rule(action, actors + optional approval requirement)
check_action(intent, optional approval, now) -> Allow | NeedsApproval | Deny
approve(intent, reviewer, now) -> opaque approval
revoke_approval(approval)
update_source_permissions(source, readers)
impact(artifact) -> affected artifact IDs, including that artifact
```

See the runnable [`document_flow`](../crates/trustgraph-framework/examples/document_flow.rs)
and [`data_export`](../crates/trustgraph-framework/examples/data_export.rs) examples.
The SDK deliberately does not dispatch an external action: the trusted effect
adapter must recheck and perform it, handling uncertain outcomes itself.

`Graph` owns its evidence and exposes immutable views. There is no unchecked
snapshot import or automatic hydration from arbitrary JSON. `finish_execution`
is a one-shot commit attempt: invalid output admission consumes its ticket,
without partially registering outputs. A source's changed bytes create a new
artifact identity; existing executions still refer to the old bytes they used.
Changing a source's permissions is an explicit operation on that exact version.
Replacing a source with newer data does not rewrite history or automatically
rerun the application.

Action decisions bind the exact intent, relevant source permission revisions and
the action rule. An unrelated source change does not stale that context. The
`impact` method scans forward dependencies; it is not an indexed distributed
incremental engine. Traversal has bounded resource limits rather than an
unbounded guarantee for every graph size.

## Interactive web demo

```sh
cargo run --locked --example framework_server
```

Open `http://127.0.0.1:9082`. The same assets are deployed at the Vercel root;
`/story.html` preserves the previous guided gateway demo. `POST /api/framework`
reconstructs a bounded fictional graph and evaluates it using this SDK on each
request. Browser controls select a scenario, resolved audience, approval and
one change. There is no shared user state, model call or external write.

The document flow demonstrates inherited restrictions, mandatory review, stale
approval after a relevant ACL revision or output change, and independence from
an unrelated ACL change. The export flow has two outputs and an action that
needs no approval. Input schema mismatch and forbidden executor access are
rejected before the simulated execution. The ACL-change scenario intentionally
keeps Finance permitted: it separates *access still allowed* from *old approval
no longer applies*. No JavaScript function invents an authorization verdict.

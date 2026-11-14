# Hydra adapter and qualification

The implemented integration is an evidence projection, with the local durable
journal as authority. No running Hydra instance was available during development;
the real integration test is intentionally ignored in the default suite.

The adapter was checked against the upstream [HTTP usage guide](https://github.com/hydra-db/hydradb/blob/main/USING-HYDRADB.md)
and [Cypher contract](https://github.com/hydra-db/hydradb/blob/main/cypher-compat.md).
It uses the HTTP query endpoint with bearer authentication, `cell_id`, named
parameters, typed result cells and the singular `bookmark` field. Remote endpoints
require HTTPS. The current configuration assumes a single server namespace;
set `TRUSTGRAPH_HYDRA_NAMESPACE` for the CLI or use `with_namespace` for explicit `X-Graph-Namespace` configuration.

Pagination preserves the original query, parameters and page size plus the query
identity and consuming cursor. Constant cursor values are permitted. A failed
page is not retried; partial results are discarded. Malformed cells/envelopes,
shape changes, stalled pages and fixed resource bounds produce failure.

## Graph shape

The projection creates immutable records for artifacts, source/ACL revisions,
transformations, each input occurrence, audience, policy, principals, approval,
publication context and the portable bundle. The explicit logical-to-numeric ID
registry assigns a caller-owned contiguous range. Each input occurrence has its
own node even when the same source is used twice.

Typed edges connect inputs to artifacts and transformations, transformations to
outputs, sources to ACLs, approvals to contexts, and contexts to exact artifact,
audience and policy. Each record stores a canonical body and digest.

Before writing, every allocated ID is checked for conflict. The adapter uses
parameterized batch node upserts and the documented batch relationship form.
After writing, every expected node and typed relationship is read back and
compared. A completion marker is written last and never substitutes for full
readback validation. Multi-request transactions are not assumed. A partially
written projection cannot authorize publication.

## Run against a real server

Use a development graph and an ID range allocated solely to this bundle. The
roundtrip intentionally retains evidence; it does not delete existing graph data.
Use the same range only when retrying the exact same exported bundle.

```sh
cargo run --locked -p trustgraph-cli -- demo --export /tmp/trustgraph-bundle.json
# Set TRUSTGRAPH_HYDRA_TOKEN in the environment using your normal secret workflow.
cargo run --locked -p trustgraph-cli -- hydra-roundtrip \
  --bundle /tmp/trustgraph-bundle.json \
  --url http://127.0.0.1:8443 --graph default --cell cell-0 \
  --base-id 1000000
```

The base ID is an example, not an assertion that the range is free. The token is
read only from the environment, not a CLI argument or checked-in configuration.

The opt-in test requires `TRUSTGRAPH_HYDRA_URL`, `TRUSTGRAPH_HYDRA_TOKEN` and
`TRUSTGRAPH_HYDRA_BASE_ID`. Optional graph/cell variables are
`TRUSTGRAPH_HYDRA_GRAPH` and `TRUSTGRAPH_HYDRA_CELL`.

```sh
cargo test --locked -p trustgraph-runtime --test hydra_contract \
  real_hydra_projection_roundtrip_is_idempotent_and_complete -- --ignored
```

## Remaining gates

Qualify the selected server revision for batch upsert, relationship retry,
bookmarks across nodes, consuming pagination and interrupted mutations. Historical read-from-graph bundle reconstruction is implemented: `load_bundle` parses the remote root, rebuilds the expected projection, validates every record/relation and requires the completion marker. Missing or modified evidence fails closed. This does not make Hydra a current ACL feed or the journal authority. Check mutation atomicity options against the
selected deployment rather than inferring them from a mock or another database.
Do not promote the integration to live-qualified from HTTP fixture tests.

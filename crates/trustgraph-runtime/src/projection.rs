//! Connected, replayable Hydra evidence projection. The local journal remains
//! the decision authority until deployment-specific contracts are qualified.
use crate::{
    Error,
    hydra::{Cell, HydraClient},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use trustgraph_core::{
    bundle::{self, Bundle},
    digest,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub id: u64,
    pub logical: String,
    pub kind: String,
    pub body: String,
    pub digest: String,
    pub bundle: String,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Edge {
    pub src: u64,
    pub dst: u64,
    pub relation: Relation,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Relation {
    UsesArtifact,
    InputOf,
    Outputs,
    HasAcl,
    SourceArtifact,
    Approves,
    ContextArtifact,
    ContextAudience,
    ContextPolicy,
    Includes,
}
impl Relation {
    fn name(self) -> &'static str {
        match self {
            Self::UsesArtifact => "USES_ARTIFACT",
            Self::InputOf => "INPUT_OF",
            Self::Outputs => "OUTPUTS",
            Self::HasAcl => "HAS_ACL",
            Self::SourceArtifact => "SOURCE_ARTIFACT",
            Self::Approves => "APPROVES",
            Self::ContextArtifact => "CONTEXT_ARTIFACT",
            Self::ContextAudience => "CONTEXT_AUDIENCE",
            Self::ContextPolicy => "CONTEXT_POLICY",
            Self::Includes => "INCLUDES",
        }
    }
    /// Binds existing records and merges the edge, so a retried projection
    /// neither duplicates relationships nor creates anonymous endpoint nodes.
    fn edge_query(self) -> String {
        format!(
            "UNWIND $rows AS row MATCH (a:TrustGraphRecord {{id: row.src}}), (b:TrustGraphRecord {{id: row.dst}}) MERGE (a)-[:{}]->(b)",
            self.name()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::Relation;
    #[test]
    fn edge_writes_match_existing_records_and_merge_relationships() {
        let query = Relation::Approves.edge_query();
        assert!(query.contains("MATCH (a:TrustGraphRecord {id: row.src})"));
        assert!(query.contains("MERGE (a)-[:APPROVES]->(b)"));
        assert!(!query.contains("CREATE"));
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Projection {
    pub registry: BTreeMap<String, u64>,
    pub nodes: Vec<Node>,
    pub edges: BTreeSet<Edge>,
    pub marker: Node,
}

fn key(sort: &str, id: impl Serialize) -> String {
    serde_json::to_string(&(sort, id)).expect("logical identity")
}

pub fn build(bundle: &Bundle, base: u64) -> Result<Projection, Error> {
    bundle::replay(bundle).map_err(|s| Error::Invalid(s.into()))?;
    let mut values = BTreeMap::<String, (String, Value)>::new();
    let mut links = vec![];
    let mut put = |sort: &str, id: String, body: Value| {
        let k = key(sort, id);
        values.insert(k.clone(), (sort.into(), body));
        k
    };
    let root = put("bundle", bundle.digest.0.clone(), json!(bundle));
    let context = bundle.decision.context().ok_or(Error::Conflict)?;
    let context_key = put("context", context.digest().0, json!(context));
    let policy_key = put("policy", "current".into(), json!(bundle.evidence.policy));
    links.push((context_key.clone(), policy_key, Relation::ContextPolicy));
    for a in bundle.evidence.artifacts.values() {
        put("artifact", a.id.0.clone(), json!(a));
    }
    for a in bundle.evidence.acls.values() {
        put("acl", a.id.0.clone(), json!(a));
    }
    for s in bundle.evidence.sources.values() {
        let k = put("source", s.id.0.clone(), json!(s));
        links.push((k.clone(), key("acl", &s.acl), Relation::HasAcl));
        links.push((k, key("artifact", &s.artifact), Relation::SourceArtifact));
    }
    for t in bundle.evidence.transformations.values() {
        let k = put("transformation", t.id.0.clone(), json!(t));
        links.push((k.clone(), key("artifact", &t.output), Relation::Outputs));
        for input in &t.manifest.inputs {
            let incidence = put(
                "input-use",
                serde_json::to_string(&(&t.id, &input.occurrence)).unwrap(),
                json!(input),
            );
            links.push((incidence.clone(), k.clone(), Relation::InputOf));
            links.push((
                incidence,
                key("artifact", &input.artifact),
                Relation::UsesArtifact,
            ));
        }
    }
    for audience in bundle.evidence.audiences.values() {
        put("audience", audience.id.0.clone(), json!(audience));
    }
    for p in bundle.evidence.principals.values() {
        put("principal", p.id.0.clone(), json!(p));
    }
    for approval in bundle.evidence.approvals.values() {
        let k = put("approval", approval.id.0.clone(), json!(approval));
        links.push((k, context_key.clone(), Relation::Approves));
    }
    links.push((
        context_key.clone(),
        key("artifact", &context.artifact),
        Relation::ContextArtifact,
    ));
    links.push((
        context_key,
        key("audience", &context.audience.id),
        Relation::ContextAudience,
    ));
    if base == 0
        || base
            .checked_add(values.len() as u64 + 1)
            .is_none_or(|end| end > i64::MAX as u64)
    {
        return Err(Error::Invalid(
            "projection ID range exceeds signed numeric IDs".into(),
        ));
    }
    let registry: BTreeMap<_, _> = values
        .keys()
        .enumerate()
        .map(|(offset, k)| (k.clone(), base + offset as u64))
        .collect();
    let nodes: Vec<_> = values
        .into_iter()
        .map(|(logical, (kind, body))| {
            let body = body.to_string();
            Node {
                id: registry[&logical],
                digest: digest::structured("graph-record", &(&logical, &kind, &body)).0,
                logical,
                kind,
                body,
                bundle: bundle.digest.0.clone(),
            }
        })
        .collect();
    for k in registry.keys().filter(|k| **k != root) {
        links.push((root.clone(), k.clone(), Relation::Includes));
    }
    let edges = links
        .into_iter()
        .map(|(src, dst, relation)| {
            Ok(Edge {
                src: *registry.get(&src).ok_or(Error::Conflict)?,
                dst: *registry.get(&dst).ok_or(Error::Conflict)?,
                relation,
            })
        })
        .collect::<Result<BTreeSet<_>, Error>>()?;
    let body = json!({"registry":registry,"nodes_digest":digest::structured("projection-nodes", &nodes),"edges_digest":digest::structured("projection-edges", &edges)}).to_string();
    let marker = Node {
        id: base + nodes.len() as u64,
        logical: key("completion", &bundle.digest),
        kind: "completion".into(),
        digest: digest::structured("projection-completion", &body).0,
        body,
        bundle: bundle.digest.0.clone(),
    };
    Ok(Projection {
        registry,
        nodes,
        edges,
        marker,
    })
}

impl HydraClient {
    /// Reconstruct historical evidence from remote records, then re-read every
    /// expected node/edge and completion marker. This never asserts current ACLs.
    pub async fn load_bundle(
        &mut self,
        digest: &digest::Digest,
        base: u64,
    ) -> Result<Bundle, Error> {
        let result=self.query("MATCH (n:TrustGraphRecord) WHERE n.bundle = $bundle AND n.kind = $kind RETURN n.id AS id, n.logical AS logical, n.kind AS kind, n.body AS body, n.digest AS digest, n.bundle AS bundle",json!({"bundle":digest.0,"kind":"bundle"}),64).await?;
        if result.columns != ["id", "logical", "kind", "body", "digest", "bundle"]
            || result.rows.len() != 1
        {
            return Err(Error::Storage(
                "missing or ambiguous Hydra bundle root".into(),
            ));
        }
        let root = decode_node(&result.rows[0])?;
        let bundle: Bundle = serde_json::from_str(&root.body)
            .map_err(|_| Error::Storage("invalid Hydra bundle body".into()))?;
        if bundle.digest != *digest {
            return Err(Error::Storage("Hydra bundle identity mismatch".into()));
        }
        let expected = build(&bundle, base)?;
        self.validate_projection(&expected, true).await?;
        Ok(bundle)
    }
    /// Writes only into the explicitly allocated numeric range. Existing IDs
    /// must already contain identical records; retries cannot overwrite evidence.
    pub async fn project(&mut self, projection: &Projection) -> Result<(), Error> {
        for node in projection
            .nodes
            .iter()
            .chain(std::iter::once(&projection.marker))
        {
            let found = self.read_node(node.id).await?;
            if found.as_ref().is_some_and(|existing| existing != node) {
                return Err(Error::Conflict);
            }
        }
        for chunk in projection.nodes.chunks(512) {
            self.write_nodes(chunk).await?;
        }
        let relations: BTreeSet<_> = projection.edges.iter().map(|e| e.relation).collect();
        for relation in relations {
            let edges: Vec<_> = projection
                .edges
                .iter()
                .filter(|e| e.relation == relation)
                .collect();
            for chunk in edges.chunks(512) {
                let rows: Vec<_> = chunk
                    .iter()
                    .map(|edge| json!({"src":edge.src,"dst":edge.dst}))
                    .collect();
                // Relation identifiers are a closed enum; only values are parameters.
                self.mutate(&relation.edge_query(), json!({"rows":rows}))
                    .await?;
            }
        }
        self.validate_projection(projection, false).await?;
        self.write_nodes(std::slice::from_ref(&projection.marker))
            .await?;
        self.validate_projection(projection, true).await
    }
    async fn write_nodes(&mut self, nodes: &[Node]) -> Result<(), Error> {
        self.mutate("UNWIND $rows AS row MERGE (n {id: row.id}) SET n:TrustGraphRecord, n.logical = row.logical, n.kind = row.kind, n.body = row.body, n.digest = row.digest, n.bundle = row.bundle",json!({"rows":nodes})).await
    }
    async fn read_node(&mut self, id: u64) -> Result<Option<Node>, Error> {
        let result = self.query("MATCH (n {id: $id}) RETURN n.id AS id, n.logical AS logical, n.kind AS kind, n.body AS body, n.digest AS digest, n.bundle AS bundle",json!({"id":id}),64).await?;
        if result.columns != ["id", "logical", "kind", "body", "digest", "bundle"] {
            return Err(Error::Storage("Hydra node columns mismatch".into()));
        }
        match result.rows.as_slice() {
            [] => Ok(None),
            [row] => Ok(Some(decode_node(row)?)),
            _ => Err(Error::Storage("ambiguous node identity".into())),
        }
    }
    /// Does not trust a completion marker without checking every node and edge.
    pub async fn validate_projection(
        &mut self,
        projection: &Projection,
        marker_required: bool,
    ) -> Result<(), Error> {
        for expected in &projection.nodes {
            if self.read_node(expected.id).await?.as_ref() != Some(expected) {
                return Err(Error::Storage("incomplete Hydra evidence nodes".into()));
            }
        }
        let relations: BTreeSet<_> = projection.edges.iter().map(|e| e.relation).collect();
        let mut observed = BTreeSet::new();
        for relation in relations {
            let query = format!(
                "MATCH (a:TrustGraphRecord)-[:{}]->(b) WHERE a.bundle = $bundle RETURN a.id AS src, b.id AS dst ORDER BY src, dst",
                relation.name()
            );
            let result = self
                .query(&query, json!({"bundle":projection.marker.bundle}), 64)
                .await?;
            if result.columns != ["src", "dst"] {
                return Err(Error::Storage("Hydra edge columns mismatch".into()));
            }
            for row in result.rows {
                let [Cell::VertexId(src), Cell::VertexId(dst)] = row.as_slice() else {
                    return Err(Error::Storage("null or invalid graph reference".into()));
                };
                if !observed.insert(Edge {
                    src: *src,
                    dst: *dst,
                    relation,
                }) {
                    return Err(Error::Storage("duplicate projected relation".into()));
                }
            }
        }
        if observed != projection.edges {
            return Err(Error::Storage(
                "incomplete Hydra evidence relationships".into(),
            ));
        }
        if marker_required
            && self.read_node(projection.marker.id).await?.as_ref() != Some(&projection.marker)
        {
            return Err(Error::Storage("missing bundle completion marker".into()));
        }
        Ok(())
    }
}
fn decode_node(row: &[Cell]) -> Result<Node, Error> {
    let [
        Cell::VertexId(id),
        Cell::String(logical),
        Cell::String(kind),
        Cell::String(body),
        Cell::String(digest),
        Cell::String(bundle),
    ] = row
    else {
        return Err(Error::Storage("incomplete or invalid Hydra node".into()));
    };
    Ok(Node {
        id: *id,
        logical: logical.clone(),
        kind: kind.clone(),
        body: body.clone(),
        digest: digest.clone(),
        bundle: bundle.clone(),
    })
}

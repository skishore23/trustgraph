use crate::*;
use reqwest::Client;

#[derive(Clone)]
pub struct Hydra {
    client: Client,
    key: String,
    base: String,
    pub database: String,
    collection: String,
}
impl Hydra {
    pub fn local() -> Result<Self> {
        let path = std::env::var("TRUSTGRAPH_HYDRA_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                PathBuf::from(std::env::var("HOME").unwrap_or_default())
                    .join(".hydradb/config.json")
            });
        let c: Value = serde_json::from_slice(&fs::read(path)?)?;
        let base = c["base_url"]
            .as_str()
            .unwrap_or("https://api.hydradb.com")
            .trim_end_matches('/')
            .to_owned();
        if base != "https://api.hydradb.com" {
            return Err(err(
                "only the configured official HydraDB HTTPS service is supported",
            ));
        }
        Ok(Self {
            client: Client::builder()
                .timeout(Duration::from_secs(35))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            key: c["api_key"]
                .as_str()
                .ok_or_else(|| err("HydraDB login required"))?
                .to_owned(),
            database: c["database"]
                .as_str()
                .ok_or_else(|| err("HydraDB database required"))?
                .to_owned(),
            base,
            collection: "trustgraph-governance".into(),
        })
    }
    pub async fn query(&self, query: &str, params: Value) -> Result<Value> {
        let response=self.client.post(format!("{}/byog/query",self.base)).bearer_auth(&self.key).header("API-Version","2").json(&json!({"database":self.database,"collection":self.collection,"query":query,"params":params})).send().await?;
        if !response.status().is_success() {
            return Err(err(&format!(
                "HydraDB returned HTTP {}",
                response.status().as_u16()
            )));
        }
        let v: Value = response.json().await?;
        if v["success"] != true || !v["data"].is_array() {
            return Err(err("HydraDB returned an invalid graph result"));
        }
        Ok(v["data"].clone())
    }
    pub async fn knowledge_progress(&self, ids: &[String]) -> Result<Vec<Value>> {
        let mut params = vec![
            ("database", self.database.clone()),
            ("collection", self.collection.clone()),
        ];
        params.extend(ids.iter().map(|id| ("ids", id.clone())));
        let response = self
            .client
            .get(format!("{}/context/status", self.base))
            .bearer_auth(&self.key)
            .header("API-Version", "2")
            .query(&params)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(err("HydraDB Knowledge status check failed"));
        }
        let v: Value = response.json().await?;
        Ok(knowledge_rows(&v)
            .iter()
            .cloned()
            .map(normalize_knowledge_status)
            .collect())
    }
    pub async fn knowledge_status(&self, id: &str) -> Result<String> {
        let rows = self.knowledge_progress(&[id.to_owned()]).await?;
        Ok(rows
            .iter()
            .find(|row| row["id"] == id)
            .and_then(|row| row["indexing_status"].as_str())
            .unwrap_or("not_registered")
            .to_owned())
    }
    pub async fn check(&self) -> Result<()> {
        let r = self.query("RETURN 1 AS connected", json!({})).await?;
        if r[0]["connected"] != 1 {
            return Err(err("HydraDB graph check failed"));
        }
        Ok(())
    }
    pub async fn put(
        &self,
        kind: &str,
        body: &impl Serialize,
        parents: &[String],
    ) -> Result<String> {
        let body = serde_json::to_string(body)?;
        if body.len() > 200_000 {
            return Err(err("graph record exceeds the bounded request size"));
        }
        let id = format!("{kind}-{}", digest::content(body.as_bytes()).0);
        self.query(
            "MERGE (n:TrustGraphRecord {id:$id}) SET n.kind=$kind,n.body=$body RETURN n.id AS id",
            json!({"id":id,"kind":kind,"body":body}),
        )
        .await?;
        if self.get(&id).await? != body {
            return Err(err("HydraDB record readback mismatch"));
        }
        for parent in parents {
            let linked=self.query("MATCH (a:TrustGraphRecord {id:$parent}),(b:TrustGraphRecord {id:$id}) MERGE (a)-[:USED_AS_INPUT]->(b) RETURN b.id AS id",json!({"parent":parent,"id":id})).await?;
            if linked.as_array().is_none_or(|r| r.len() != 1) || linked[0]["id"] != id {
                return Err(err("HydraDB did not retain a required input relationship"));
            }
        }
        Ok(id)
    }
    pub async fn validate_edges(&self, id: &str, parents: &[String]) -> Result<()> {
        self.get(id).await?;
        let rows=self.query("MATCH (a:TrustGraphRecord)-[:USED_AS_INPUT]->(b:TrustGraphRecord {id:$id}) RETURN a.id AS id",json!({"id":id})).await?;
        let actual: BTreeSet<_> = rows
            .as_array()
            .ok_or_else(|| err("invalid graph relationships"))?
            .iter()
            .map(|r| r["id"].as_str().unwrap_or("").to_owned())
            .collect();
        if actual != parents.iter().cloned().collect() {
            return Err(err(
                "HydraDB input relationships no longer match the captured lineage",
            ));
        }
        Ok(())
    }
    pub async fn get(&self, id: &str) -> Result<String> {
        let rows = self
            .query(
                "MATCH (n:TrustGraphRecord {id:$id}) RETURN n.body AS body",
                json!({"id":id}),
            )
            .await?;
        if rows.as_array().is_none_or(|r| r.len() != 1) {
            return Err(err("HydraDB evidence is missing or ambiguous"));
        }
        let body = rows[0]["body"]
            .as_str()
            .ok_or_else(|| err("HydraDB body is missing"))?
            .to_owned();
        if !id.ends_with(&digest::content(body.as_bytes()).0) {
            return Err(err("HydraDB evidence fingerprint changed"));
        }
        Ok(body)
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct GraphAuthorization {
    bundle_node: String,
    nodes: Vec<String>,
    edges: Vec<(String, String, String)>,
}
impl Hydra {
    pub async fn store_authorization(
        &self,
        bundle: &trustgraph_core::bundle::Bundle,
        parents: &[String],
    ) -> Result<String> {
        let projection = trustgraph_runtime::projection::build(bundle, 1)?;
        let namespace = digest::structured("authorization-graph", bundle).0;
        let mut ids = BTreeMap::new();
        let mut rows = Vec::new();
        let mut bundle_node = None;
        for node in &projection.nodes {
            let body = serde_json::to_string(node)?;
            if body.len() > 200_000 {
                return Err(err("authorization record exceeds graph request bound"));
            }
            let id = format!(
                "{}-{}-{}",
                node.kind,
                namespace,
                digest::content(body.as_bytes()).0
            );
            ids.insert(node.id, id.clone());
            if node.kind == "bundle" {
                bundle_node = Some(id.clone());
            }
            rows.push(json!({"id":id,"kind":node.kind,"body":body}));
        }
        for batch in rows.chunks(12) {
            self.query("UNWIND $rows AS row MERGE (n:TrustGraphRecord {id:row.id}) SET n.kind=row.kind,n.body=row.body RETURN n.id AS id",json!({"rows":batch})).await?;
        }
        let mut edges = Vec::new();
        for edge in &projection.edges {
            let relation = serde_json::to_value(edge.relation)?
                .as_str()
                .ok_or_else(|| err("relation name unavailable"))?
                .to_owned();
            edges.push((ids[&edge.src].clone(), ids[&edge.dst].clone(), relation));
        }
        let index = GraphAuthorization {
            bundle_node: bundle_node.ok_or_else(|| err("authorization bundle missing"))?,
            nodes: ids.values().cloned().collect(),
            edges,
        };
        let mut groups: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        for (src, dst, relation) in &index.edges {
            groups
                .entry(relation.clone())
                .or_default()
                .push(json!({"src":src,"dst":dst}));
        }
        for (relation, edges) in groups {
            if !relation
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b == b'_')
            {
                return Err(err("invalid relation name"));
            }
            let query = format!(
                "UNWIND $edges AS edge MATCH (a:TrustGraphRecord {{id:edge.src}}),(b:TrustGraphRecord {{id:edge.dst}}) MERGE (a)-[:{relation}]->(b) RETURN a.id AS src,b.id AS dst"
            );
            let written = self.query(&query, json!({"edges":edges})).await?;
            if written.as_array().is_none_or(|r| r.len() != edges.len()) {
                return Err(err("incomplete authorization graph relationships"));
            }
        }
        let id = self.put("authorization", &index, parents).await?;
        self.verify_authorization(&id).await?;
        Ok(id)
    }
    pub async fn verify_authorization(&self, id: &str) -> Result<trustgraph_core::bundle::Bundle> {
        let index: GraphAuthorization = serde_json::from_str(&self.get(id).await?)?;
        let node: trustgraph_runtime::projection::Node =
            serde_json::from_str(&self.get(&index.bundle_node).await?)?;
        let bundle: trustgraph_core::bundle::Bundle = serde_json::from_str(&node.body)?;
        trustgraph_core::bundle::replay(&bundle).map_err(err)?;
        let projection = trustgraph_runtime::projection::build(&bundle, 1)?;
        let namespace = digest::structured("authorization-graph", &bundle).0;
        let mut expected_nodes = BTreeMap::new();
        let mut by_numeric = BTreeMap::new();
        for node in projection.nodes {
            let body = serde_json::to_string(&node)?;
            let key = format!(
                "{}-{}-{}",
                node.kind,
                namespace,
                digest::content(body.as_bytes()).0
            );
            by_numeric.insert(node.id, key.clone());
            expected_nodes.insert(key, body);
        }
        if index.nodes.iter().cloned().collect::<BTreeSet<_>>()
            != expected_nodes.keys().cloned().collect()
        {
            return Err(err("authorization graph registry mismatch"));
        }
        let expected_edges: BTreeSet<_> = projection
            .edges
            .iter()
            .map(|e| {
                (
                    by_numeric[&e.src].clone(),
                    by_numeric[&e.dst].clone(),
                    serde_json::to_value(e.relation)
                        .expect("relation")
                        .as_str()
                        .expect("relation name")
                        .to_owned(),
                )
            })
            .collect();
        if expected_edges != index.edges.iter().cloned().collect() {
            return Err(err("authorization graph edge index mismatch"));
        }
        for batch in index.nodes.chunks(12) {
            let actual=self.query("MATCH (n:TrustGraphRecord) WHERE n.id IN $ids RETURN n.id AS id,n.body AS body",json!({"ids":batch})).await?;
            let mut found = BTreeSet::new();
            for row in actual
                .as_array()
                .ok_or_else(|| err("invalid graph records"))?
            {
                let key = row["id"].as_str().ok_or_else(|| err("graph ID missing"))?;
                if expected_nodes.get(key).map(String::as_str) != row["body"].as_str() {
                    return Err(err("authorization graph record was modified"));
                }
                found.insert(key);
            }
            if found.len() != batch.len() {
                return Err(err("authorization graph record missing"));
            }
        }
        let mut groups: BTreeMap<String, BTreeSet<(String, String)>> = BTreeMap::new();
        for (a, b, r) in &expected_edges {
            groups
                .entry(r.clone())
                .or_default()
                .insert((a.clone(), b.clone()));
        }
        for (relation, expected) in groups {
            if !relation
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b == b'_')
            {
                return Err(err("invalid relation name"));
            }
            let query = format!(
                "MATCH (a:TrustGraphRecord)-[:{relation}]->(b:TrustGraphRecord) WHERE a.id IN $ids RETURN a.id AS src,b.id AS dst"
            );
            let actual = self.query(&query, json!({"ids":index.nodes})).await?;
            let found: BTreeSet<_> = actual
                .as_array()
                .ok_or_else(|| err("invalid graph edges"))?
                .iter()
                .map(|r| {
                    (
                        r["src"].as_str().unwrap_or("").to_owned(),
                        r["dst"].as_str().unwrap_or("").to_owned(),
                    )
                })
                .collect();
            if found != expected {
                return Err(err("authorization graph relationship missing or modified"));
            }
        }
        Ok(bundle)
    }
}

fn normalize_knowledge_status(mut row: Value) -> Value {
    // The hosted API reports an unregistered optional Knowledge ID as FILE_NOT_FOUND,
    // not as an indexing attempt that failed. Preserve its diagnostic, distinguish the state.
    if row["success"] == false && row["error_code"] == "FILE_NOT_FOUND" {
        row["indexing_status"] = json!("not_registered");
    }
    row
}
fn knowledge_rows(v: &Value) -> &[Value] {
    let v = v.get("data").unwrap_or(v);
    v["statuses"].as_array().map(Vec::as_slice).unwrap_or(&[])
}
#[cfg(test)]
mod knowledge_tests {
    use super::*;
    #[test]
    fn hosted_status_envelope_preserves_pending_completed_and_errors() {
        let v = json!({"success":true,"data":{"statuses":[
            {"id":"pending-source","indexing_status":"queued"},
            {"id":"ready-source","indexing_status":"completed"},
            {"id":"failed-source","indexing_status":"errored","error_message":"indexing failed"}
        ]}});
        let rows = knowledge_rows(&v);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["indexing_status"], "queued");
        assert_eq!(rows[1]["indexing_status"], "completed");
        assert_eq!(rows[2]["error_message"], "indexing failed");
        assert!(knowledge_rows(&json!({"success":false})).is_empty());
        let absent = normalize_knowledge_status(
            json!({"id":"absent","success":false,"error_code":"FILE_NOT_FOUND","indexing_status":"errored"}),
        );
        assert_eq!(absent["indexing_status"], "not_registered");
        assert_eq!(
            normalize_knowledge_status(rows[2].clone())["indexing_status"],
            "errored"
        );
    }
}

//! Conservative Hydra HTTP transport. Graph results never bypass the Rust gate.
use crate::{Error, journal::storage};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Cell {
    VertexId(u64),
    Integer(i64),
    String(String),
    Boolean(bool),
    Null,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rows {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Cell>>,
}
#[derive(Deserialize)]
struct Page {
    query_id: String,
    columns: Vec<String>,
    rows: Vec<Vec<Cell>>,
    next_cursor: Option<u64>,
    bookmark: Option<String>,
}

pub struct HydraClient {
    http: Client,
    endpoint: Url,
    cell: String,
    token: String,
    bookmark: Option<String>,
    response_bookmark: Option<String>,
    namespace: Option<String>,
}
impl HydraClient {
    pub fn new(url: &str, graph: &str, cell: &str, token: &str) -> Result<Self, Error> {
        let mut endpoint = Url::parse(url).map_err(storage)?;
        if !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(Error::Invalid(
                "Hydra URL must not contain credentials, query or fragment".into(),
            ));
        }
        if endpoint.scheme() != "https"
            && !(endpoint.scheme() == "http"
                && matches!(
                    endpoint.host_str(),
                    Some("localhost" | "127.0.0.1" | "[::1]")
                ))
        {
            return Err(Error::Invalid("remote Hydra requires HTTPS".into()));
        }
        if graph.is_empty()
            || !graph
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            || cell.is_empty()
            || token.is_empty()
        {
            return Err(Error::Invalid("graph, cell and token are required".into()));
        }
        endpoint.set_path(&format!("/v1/graphs/{graph}/query"));
        let http = Client::builder()
            .timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(storage)?;
        Ok(Self {
            http,
            endpoint,
            cell: cell.into(),
            token: token.into(),
            bookmark: None,
            response_bookmark: None,
            namespace: None,
        })
    }
    pub fn with_namespace(mut self, namespace: &str) -> Result<Self, Error> {
        if namespace.is_empty()
            || namespace.len() > 128
            || !namespace
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        {
            return Err(Error::Invalid("invalid Hydra namespace".into()));
        }
        self.namespace = Some(namespace.into());
        Ok(self)
    }
    pub fn bookmark(&self) -> Option<&str> {
        self.bookmark.as_deref()
    }
    /// Fetch every page once. A consuming cursor is never retried after failure.
    /// Returned rows are complete or the entire request fails.
    pub async fn query(
        &mut self,
        query: &str,
        parameters: Value,
        page_size: u32,
    ) -> Result<Rows, Error> {
        self.response_bookmark = None;
        if !(1..=4096).contains(&page_size) {
            return Err(Error::Invalid("page size outside Hydra limit".into()));
        }
        let mut request = json!({"cell_id":self.cell, "query":query, "parameters":parameters, "page_size":page_size, "consistency":"causal"});
        if let Some(bookmark) = &self.bookmark {
            request["bookmark"] = json!(bookmark);
        }
        let mut result = Rows {
            columns: vec![],
            rows: vec![],
        };
        let mut query_id = None;
        let mut previous_page = None;
        for _ in 0..128 {
            let mut outbound = self
                .http
                .post(self.endpoint.clone())
                .bearer_auth(&self.token)
                .json(&request);
            if let Some(namespace) = &self.namespace {
                outbound = outbound.header("X-Graph-Namespace", namespace);
            }
            let response = outbound
                .send()
                .await
                .map_err(storage)?
                .error_for_status()
                .map_err(storage)?;
            let value: Value = response.json().await.map_err(storage)?;
            if value.get("error").is_some() || value.get("next_cursor").is_none() {
                return Err(Error::Storage("invalid Hydra result envelope".into()));
            }
            let page: Page = serde_json::from_value(value).map_err(storage)?;
            if page.query_id.is_empty()
                || page.rows.iter().any(|row| row.len() != page.columns.len())
            {
                return Err(Error::Storage("invalid Hydra row shape".into()));
            }
            if let Some(id) = &query_id {
                if *id != page.query_id || result.columns != page.columns {
                    return Err(Error::Storage("Hydra pagination identity changed".into()));
                }
            } else {
                query_id = Some(page.query_id.clone());
                result.columns = page.columns;
            }
            let signature = trustgraph_core::digest::structured("hydra-page", &page.rows);
            if page.next_cursor.is_some()
                && (page.rows.is_empty() || previous_page.as_ref() == Some(&signature))
            {
                return Err(Error::Storage("Hydra pagination did not advance".into()));
            }
            previous_page = Some(signature);
            result.rows.extend(page.rows);
            if result.rows.len() > 65_536 {
                return Err(Error::Storage("Hydra result limit exceeded".into()));
            }
            self.response_bookmark = page.bookmark.clone();
            if let Some(bookmark) = page.bookmark {
                self.bookmark = Some(bookmark);
            }
            match page.next_cursor {
                None => return Ok(result),
                Some(cursor) => {
                    request["cursor"] = json!(cursor);
                    request["query_id"] = json!(page.query_id);
                }
            }
        }
        Err(Error::Storage(
            "Hydra page bound exceeded; incomplete result discarded".into(),
        ))
    }
    pub async fn mutate(&mut self, query: &str, parameters: Value) -> Result<(), Error> {
        let rows = self.query(query, parameters, 64).await?;
        if !rows.columns.is_empty() || !rows.rows.is_empty() || self.response_bookmark.is_none() {
            return Err(Error::Storage(
                "mutation lacked expected acknowledgment/bookmark".into(),
            ));
        }
        Ok(())
    }
}

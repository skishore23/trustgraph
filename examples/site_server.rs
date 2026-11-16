//! Local equivalent of the hosted site: the policy playground page from `web/`
//! and `POST /api/policy`. Pages are read from disk on each request, so edits
//! show on refresh.
use std::path::PathBuf;

use axum::{
    Router,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};

async fn asset(name: &'static str) -> Response {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("web")
        .join(name);
    let Ok(text) = tokio::fs::read_to_string(path).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let kind = match name.rsplit('.').next() {
        Some("css") => "text/css",
        Some("js") => "text/javascript",
        _ => "text/html; charset=utf-8",
    };
    ([(header::CONTENT_TYPE, kind)], text).into_response()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app: Router = trustgraph_demo::router()
        .route("/", get(|| asset("index.html")))
        .route("/site.css", get(|| asset("site.css")))
        .route("/site.js", get(|| asset("site.js")));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:9082").await?;
    println!("TrustGraph site: http://127.0.0.1:9082 (fictional data; no external effects)");
    axum::serve(listener, app).await?;
    Ok(())
}

//! Local equivalent of the hosted site: the policy playground page from `web/`,
//! its self-hosted fonts and `POST /api/policy`. Files are read from disk on
//! each request, so edits show on refresh.
use std::path::PathBuf;

use axum::{
    Router,
    extract::Path,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};

async fn asset(name: String) -> Response {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("web")
        .join(&name);
    let Ok(bytes) = tokio::fs::read(path).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let kind = match name.rsplit('.').next() {
        Some("css") => "text/css",
        Some("js") => "text/javascript",
        Some("woff2") => "font/woff2",
        _ => "text/html; charset=utf-8",
    };
    ([(header::CONTENT_TYPE, kind)], bytes).into_response()
}

/// Only the four published font files, by exact name: no path traversal.
async fn font(Path(name): Path<String>) -> Response {
    let ok = name.starts_with("ibm-plex-")
        && name.ends_with(".woff2")
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
        && !name.contains("..");
    if !ok {
        return StatusCode::NOT_FOUND.into_response();
    }
    asset(format!("fonts/{name}")).await
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app: Router = trustgraph_demo::router()
        .route("/", get(|| asset("index.html".into())))
        .route("/site.css", get(|| asset("site.css".into())))
        .route("/site.js", get(|| asset("site.js".into())))
        .route("/fonts/{name}", get(font));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:9082").await?;
    println!("TrustGraph site: http://127.0.0.1:9082 (fictional data; no external effects)");
    axum::serve(listener, app).await?;
    Ok(())
}

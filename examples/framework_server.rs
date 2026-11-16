//! Local equivalent of the hosted site: the shared API router (including
//! `/api/policy`) plus the static pages from `web/`, read from disk on each
//! request so edits show on refresh. `/` serves `web/index.html` when it
//! exists, otherwise the typed-dataflow page.
use std::path::PathBuf;

use axum::{
    Router,
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::get,
};

fn web(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("web")
        .join(name)
}

async fn page(file: &'static str, fallback: Option<&'static str>) -> Response {
    let mut candidates = vec![file];
    candidates.extend(fallback);
    for name in candidates {
        if let Ok(text) = tokio::fs::read_to_string(web(name)).await {
            let text = if name == "story.html" {
                text.replace(
                    "<head>",
                    "<head><meta name=\"trustgraph-mode\" content=\"hosted\">",
                )
            } else {
                text
            };
            let kind = match name.rsplit('.').next() {
                Some("css") => "text/css",
                Some("js") => "text/javascript",
                _ => "text/html; charset=utf-8",
            };
            return ([(header::CONTENT_TYPE, kind)], Html(text)).into_response();
        }
    }
    StatusCode::NOT_FOUND.into_response()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut app: Router =
        trustgraph_demo::router().route("/", get(|| page("index.html", Some("framework.html"))));
    for name in [
        "site.css",
        "site.js",
        "framework.css",
        "framework.js",
        "story.html",
        "story.css",
        "story.js",
    ] {
        app = app.route(&format!("/{name}"), get(move || page(name, None)));
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:9082").await?;
    println!("TrustGraph site: http://127.0.0.1:9082 (fictional data; no external effects)");
    axum::serve(listener, app).await?;
    Ok(())
}

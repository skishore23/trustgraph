//! Local equivalent of the stateless Vercel typed-dataflow demonstration.
use axum::{http::header, response::Html, routing::get};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app = trustgraph_demo::router()
        .route(
            "/",
            get(|| async { Html(include_str!("../web/framework.html")) }),
        )
        .route(
            "/framework.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css")],
                    include_str!("../web/framework.css"),
                )
            }),
        )
        .route(
            "/framework.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript")],
                    include_str!("../web/framework.js"),
                )
            }),
        )
        .route(
            "/story.html",
            get(|| async {
                Html(include_str!("../web/story.html").replace(
                    "<head>",
                    "<head><meta name=\"trustgraph-mode\" content=\"hosted\">",
                ))
            }),
        )
        .route(
            "/story.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css")],
                    include_str!("../web/story.css"),
                )
            }),
        )
        .route(
            "/story.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript")],
                    include_str!("../web/story.js"),
                )
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:9082").await?;
    println!("Typed-dataflow demo: http://127.0.0.1:9082 (fictional data; no external effects)");
    axum::serve(listener, app).await?;
    Ok(())
}

//! The hosted site's API: the stateless policy playground endpoint. No
//! credentials, durable state or external writes.
pub mod errors;
pub mod policy;

use axum::{Router, extract::DefaultBodyLimit, routing::post};

/// Largest accepted JSON body for every hosted endpoint.
pub const MAX_BODY_BYTES: usize = 131_072;

pub fn router() -> Router {
    Router::new()
        .route("/api/policy", post(policy::demo))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(axum::middleware::map_response(errors::json_errors))
}

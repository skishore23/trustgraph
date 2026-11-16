//! MCP over streamable HTTP at `/mcp`.

use std::sync::Arc;

use axum::Router;

use super::{ServeJournal, Server};

pub fn router<J: ServeJournal>(_server: Arc<Server<J>>) -> Router<Arc<Server<J>>> {
    Router::new()
}

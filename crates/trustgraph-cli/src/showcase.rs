use crate::api::{Shared, now, with_world};
use axum::{
    Extension, Json, Router,
    extract::State,
    http::HeaderMap,
    response::Response,
    routing::{get, post},
};
use std::sync::{Arc, Mutex};
use trustgraph_runtime::{
    Error,
    showcase::{Command, Showcase},
};

type Sandbox = Arc<Mutex<Showcase>>;

pub(crate) fn routes() -> Result<Router<Shared>, Error> {
    Ok(Router::new()
        .route("/api/demo", get(view).post(command))
        .route("/api/demo/reset", post(reset))
        .layer(Extension(Arc::new(Mutex::new(Showcase::new()?)))))
}
async fn view(
    State(shared): State<Shared>,
    Extension(sandbox): Extension<Sandbox>,
    headers: HeaderMap,
) -> Response {
    with_world(&shared, &headers, |w, actor| {
        if w.state().evidence.workspace.0 != "bank" {
            return Err(Error::NotFound.into());
        }
        let sandbox = sandbox.lock().map_err(|_| Error::RecoveryRequired)?;
        Ok(Json(sandbox.view(&actor, now())?))
    })
}
async fn command(
    State(shared): State<Shared>,
    Extension(sandbox): Extension<Sandbox>,
    headers: HeaderMap,
    Json(command): Json<Command>,
) -> Response {
    with_world(&shared, &headers, |w, actor| {
        if w.state().evidence.workspace.0 != "bank" {
            return Err(Error::NotFound.into());
        }
        let mut sandbox = sandbox.lock().map_err(|_| Error::RecoveryRequired)?;
        Ok(Json(sandbox.execute(&actor, command, now())?))
    })
}
async fn reset(
    State(shared): State<Shared>,
    Extension(sandbox): Extension<Sandbox>,
    headers: HeaderMap,
) -> Response {
    with_world(&shared, &headers, |w, actor| {
        if w.state().evidence.workspace.0 != "bank" {
            return Err(Error::NotFound.into());
        }
        let mut sandbox = sandbox.lock().map_err(|_| Error::RecoveryRequired)?;
        *sandbox = Showcase::new()?;
        Ok(Json(sandbox.view(&actor, now())?))
    })
}

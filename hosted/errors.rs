//! Every hosted error response is a small JSON envelope, including extractor
//! rejections (malformed JSON, unknown fields, wrong content type, oversize
//! bodies), unknown routes and transport-level refusals from the MCP service.
use axum::{
    Json,
    body::to_bytes,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::json;

/// Error bodies produced by Axum and rmcp are short plain-text strings.
const MAX_ERROR_BODY: usize = 4_096;
const MAX_MESSAGE: usize = 300;

fn is_json(response: &Response) -> bool {
    response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"))
}

fn fallback_message(status: StatusCode) -> &'static str {
    match status {
        StatusCode::NOT_FOUND => "This endpoint does not exist.",
        StatusCode::METHOD_NOT_ALLOWED => "This endpoint does not accept that HTTP method.",
        StatusCode::PAYLOAD_TOO_LARGE => "The request body is too large for this demo.",
        StatusCode::UNSUPPORTED_MEDIA_TYPE => "Send the request as application/json.",
        _ => "The request could not be completed.",
    }
}

/// Rewrites any non-JSON 4xx/5xx response into `{"error": "..."}`.
pub async fn json_errors(response: Response) -> Response {
    let status = response.status();
    if !(status.is_client_error() || status.is_server_error()) || is_json(&response) {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let text = to_bytes(body, MAX_ERROR_BODY)
        .await
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| fallback_message(status).to_owned());
    let message: String = text.chars().take(MAX_MESSAGE).collect();
    parts.headers.remove(header::CONTENT_LENGTH);
    parts.headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    parts
        .headers
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    let body = Json(json!({"error": message})).into_response().into_body();
    Response::from_parts(parts, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn plain_text_and_empty_errors_become_json() {
        let plain = (StatusCode::UNPROCESSABLE_ENTITY, "Failed to deserialize").into_response();
        let response = json_errors(plain).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
        let bytes = to_bytes(response.into_body(), 10_000).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["error"], "Failed to deserialize");

        let empty = StatusCode::NOT_FOUND.into_response();
        let bytes = to_bytes(json_errors(empty).await.into_body(), 10_000)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["error"], "This endpoint does not exist.");
    }

    #[tokio::test]
    async fn successful_and_json_responses_are_untouched() {
        let ok = (StatusCode::OK, "plain").into_response();
        let bytes = to_bytes(json_errors(ok).await.into_body(), 100)
            .await
            .unwrap();
        assert_eq!(&bytes[..], b"plain");
        let json_error = (StatusCode::BAD_REQUEST, Json(json!({"error":"kept"}))).into_response();
        let bytes = to_bytes(json_errors(json_error).await.into_body(), 100)
            .await
            .unwrap();
        assert_eq!(&bytes[..], br#"{"error":"kept"}"#);
    }

    async fn call(
        method: &str,
        path: &str,
        content_type: Option<&str>,
        body: String,
    ) -> (StatusCode, serde_json::Value) {
        use tower::ServiceExt;
        let mut builder = axum::http::Request::builder().method(method).uri(path);
        if let Some(content_type) = content_type {
            builder = builder.header("content-type", content_type);
        }
        let response = crate::router()
            .oneshot(builder.body(axum::body::Body::from(body)).unwrap())
            .await
            .unwrap();
        let status = response.status();
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
        let bytes = to_bytes(response.into_body(), 100_000).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).expect("JSON error body"),
        )
    }

    #[tokio::test]
    async fn every_hosted_endpoint_returns_json_for_extractor_rejections() {
        let json = Some("application/json");
        {
            let path = "/api/policy";
            let cases = [
                (
                    "POST",
                    json,
                    "{not json".to_owned(),
                    StatusCode::BAD_REQUEST,
                ),
                (
                    "POST",
                    None,
                    "{}".to_owned(),
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                ),
                (
                    "POST",
                    json,
                    format!(r#"{{"pad":"{}"}}"#, "x".repeat(crate::MAX_BODY_BYTES)),
                    StatusCode::PAYLOAD_TOO_LARGE,
                ),
                ("GET", None, String::new(), StatusCode::METHOD_NOT_ALLOWED),
            ];
            for (method, content_type, body, expected) in cases {
                let (status, value) = call(method, path, content_type, body).await;
                assert_eq!(status, expected, "{method} {path}");
                assert!(
                    value["error"].as_str().is_some_and(|e| !e.is_empty()),
                    "{path}"
                );
            }
        }
        let (status, value) = call("GET", "/api/missing", None, String::new()).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(value["error"], "This endpoint does not exist.");
    }
}

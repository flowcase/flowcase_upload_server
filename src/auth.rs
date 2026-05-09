use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{header::AUTHORIZATION, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;

/// Tower middleware that enforces HTTP Basic auth against `expected`.
///
/// Mirrors the behavior tree of the legacy Python at
/// legacy-python/flowcase_upload_server.py:17-32, but fixes the
/// bug where a missing or non-Basic Authorization header would fall
/// through and let the upload proceed.
///
/// Error strings:
/// - missing/non-Basic header → 403 `Access Denied!`
/// - base64 decode fails       → 403 `Failed to decode auth 1`
/// - decoded value mismatch    → 403 `Access Denied!`
///
/// The Python's "Failed to decode auth 2" branch (ISO-8859-1 decode
/// failure) is unreachable — ISO-8859-1 is a single-byte encoding —
/// so we don't replicate it.
pub async fn require_basic_auth(
    State(expected): State<Arc<String>>,
    req: Request,
    next: Next,
) -> Response {
    let Some(value) = req.headers().get(AUTHORIZATION) else {
        return access_denied();
    };
    let Ok(s) = value.to_str() else {
        return access_denied();
    };
    let Some(b64) = s.strip_prefix("Basic ") else {
        return access_denied();
    };
    let Ok(raw) = B64.decode(b64.trim()) else {
        return failed_to_decode();
    };
    // Python decodes as ISO-8859-1 (lossless byte-to-codepoint). We do
    // the same so non-UTF-8 user:pass tokens behave identically.
    let decoded: String = raw.iter().map(|b| *b as char).collect();
    if decoded != *expected {
        return access_denied();
    }
    next.run(req).await
}

fn access_denied() -> Response {
    (StatusCode::FORBIDDEN, "Access Denied!").into_response()
}

fn failed_to_decode() -> Response {
    (StatusCode::FORBIDDEN, "Failed to decode auth 1").into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use axum::middleware::from_fn_with_state;
    use axum::routing::post;
    use axum::Router;
    use tower::ServiceExt;

    fn app(token: &str) -> Router {
        let expected = Arc::new(token.to_string());
        Router::new()
            .route("/upload", post(|| async { "ok" }))
            .layer(from_fn_with_state(expected, require_basic_auth))
    }

    async fn body_string(resp: Response) -> String {
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        String::from_utf8(body.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn no_auth_header_is_403() {
        let req = Request::builder()
            .method("POST")
            .uri("/upload")
            .body(Body::empty())
            .unwrap();
        let resp = app("user:pass").oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert_eq!(body_string(resp).await, "Access Denied!");
    }

    #[tokio::test]
    async fn wrong_token_is_403() {
        let bad = B64.encode(b"user:WRONG");
        let req = Request::builder()
            .method("POST")
            .uri("/upload")
            .header("Authorization", format!("Basic {bad}"))
            .body(Body::empty())
            .unwrap();
        let resp = app("user:pass").oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert_eq!(body_string(resp).await, "Access Denied!");
    }

    #[tokio::test]
    async fn correct_token_passes_through() {
        let good = B64.encode(b"user:pass");
        let req = Request::builder()
            .method("POST")
            .uri("/upload")
            .header("Authorization", format!("Basic {good}"))
            .body(Body::empty())
            .unwrap();
        let resp = app("user:pass").oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(body_string(resp).await, "ok");
    }

    #[tokio::test]
    async fn malformed_base64_returns_decode_failure() {
        let req = Request::builder()
            .method("POST")
            .uri("/upload")
            .header("Authorization", "Basic !!!not-base64!!!")
            .body(Body::empty())
            .unwrap();
        let resp = app("user:pass").oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert_eq!(body_string(resp).await, "Failed to decode auth 1");
    }

    #[tokio::test]
    async fn non_basic_scheme_is_403() {
        let req = Request::builder()
            .method("POST")
            .uri("/upload")
            .header("Authorization", "Bearer something")
            .body(Body::empty())
            .unwrap();
        let resp = app("user:pass").oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert_eq!(body_string(resp).await, "Access Denied!");
    }
}

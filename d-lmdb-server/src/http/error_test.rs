use axum::http::StatusCode;
use axum::http::header::RETRY_AFTER;
use axum::response::IntoResponse;
use d_lmdb::ClientApiError;
use d_lmdb::Error;
use d_lmdb::ErrorCode;
use d_lmdb::LeaderHint;

use super::HttpError;
use super::internal_error_category;
use super::normalize_error_body;
use super::replace_port;
use super::retry_after_ms_to_secs;

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn business(
    code: ErrorCode,
    message: &str,
) -> Error {
    Error::Client(ClientApiError::Business {
        code,
        message: message.to_string(),
        required_action: None,
    })
}

// ── 400 ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_key_too_large_maps_to_400() {
    let http_err: HttpError = Error::KeyTooLarge(600).into();
    let resp = http_err.into_response();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp).await;
    assert!(body["error"].as_str().unwrap().contains("600"));
    assert!(body.get("leader_hint").is_none());
    assert!(body.get("trace_id").is_none());
}

#[tokio::test]
async fn test_value_too_large_maps_to_400() {
    let http_err: HttpError = Error::ValueTooLarge(2_000_000).into();
    assert_eq!(http_err.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_business_invalid_request_maps_to_400() {
    let err = business(ErrorCode::InvalidRequest, "bad params");
    let http_err: HttpError = err.into();
    assert_eq!(http_err.status, StatusCode::BAD_REQUEST);
    assert_eq!(http_err.body.error, "bad params");
}

// ── 503 ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_not_leader_maps_to_503_with_leader_hint_and_retry_after() {
    let err = Error::Client(ClientApiError::Network {
        code: ErrorCode::NotLeader,
        message: "not leader".to_string(),
        retry_after_ms: Some(150),
        leader_hint: Some(LeaderHint {
            leader_id: 2,
            address: "10.0.0.2:9081".to_string(),
        }),
    });
    let http_err: HttpError = err.into();
    assert_eq!(http_err.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(http_err.retry_after_secs, Some(1)); // 150ms ceils to 1s

    let resp = http_err.into_response();
    assert_eq!(resp.headers().get(RETRY_AFTER).unwrap(), "1");
    let body = body_json(resp).await;
    assert_eq!(body["leader_hint"]["leader_id"], 2);
    assert_eq!(body["leader_hint"]["address"], "10.0.0.2:9081");
}

#[tokio::test]
async fn test_not_leader_without_hint_omits_leader_hint_field() {
    let err = Error::Client(ClientApiError::Network {
        code: ErrorCode::NotLeader,
        message: "not leader".to_string(),
        retry_after_ms: None,
        leader_hint: None,
    });
    let http_err: HttpError = err.into();
    assert_eq!(http_err.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(http_err.retry_after_secs, None);
    let resp = http_err.into_response();
    assert!(resp.headers().get(RETRY_AFTER).is_none());
    let body = body_json(resp).await;
    assert!(body.get("leader_hint").is_none());
}

#[tokio::test]
async fn test_cluster_unavailable_maps_to_503_without_leader_hint() {
    let err = business(ErrorCode::ClusterUnavailable, "cluster unavailable");
    let http_err: HttpError = err.into();
    assert_eq!(http_err.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(http_err.body.leader_hint.is_none());
}

#[tokio::test]
async fn test_term_outdated_maps_to_503() {
    // decisions/015: TermOutdated means the leader just stepped down mid-write —
    // the client should retry, this is not an unactionable 500.
    let err = business(ErrorCode::TermOutdated, "stale term error");
    let http_err: HttpError = err.into();
    assert_eq!(http_err.status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn test_propose_failed_and_retry_required_map_to_503() {
    for code in [ErrorCode::ProposeFailed, ErrorCode::RetryRequired] {
        let http_err: HttpError = business(code, "retry").into();
        assert_eq!(http_err.status, StatusCode::SERVICE_UNAVAILABLE);
    }
}

// ── 500 fallback ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_unmapped_error_falls_back_to_500_with_trace_id() {
    // decisions/015 Known Limitation: StaleOperation/RateLimited have no real
    // construction path via EmbeddedEngine — they deliberately fall through here.
    let err = business(ErrorCode::StaleOperation, "stale operation");
    let http_err: HttpError = err.into();
    assert_eq!(http_err.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(http_err.body.trace_id.is_some());

    let resp = http_err.into_response();
    let body = body_json(resp).await;
    assert_eq!(body["trace_id"].as_str().unwrap().len(), 36); // uuid v4 string length
}

#[tokio::test]
async fn test_storage_error_maps_to_500() {
    let http_err: HttpError = Error::Storage("corrupt value".to_string()).into();
    assert_eq!(http_err.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(http_err.body.error, "storage error");
}

// ── HttpError::bad_request / not_found ──────────────────────────────────────

#[test]
fn test_not_found_constructor() {
    let http_err = HttpError::not_found();
    assert_eq!(http_err.status, StatusCode::NOT_FOUND);
    assert_eq!(http_err.body.error, "key not found");
}

// ── internal_error_category ─────────────────────────────────────────────────

#[test]
fn test_internal_error_category_storage() {
    assert_eq!(
        internal_error_category(&Error::Storage("x".to_string())),
        "storage error"
    );
}

#[test]
fn test_internal_error_category_configuration() {
    assert_eq!(internal_error_category(&Error::Path("x".to_string())), "configuration error");
}

#[test]
fn test_internal_error_category_cluster() {
    let err = business(ErrorCode::Uncategorized, "x");
    assert_eq!(internal_error_category(&err), "cluster error");
}

#[test]
fn test_internal_error_category_fallback_is_internal_error() {
    let err = Error::Client(ClientApiError::Protocol {
        code: ErrorCode::InvalidResponse,
        message: "x".to_string(),
        supported_versions: None,
    });
    assert_eq!(internal_error_category(&err), "internal error");
}

// ── replace_port ─────────────────────────────────────────────────────────────
//
// Pure function, no LOCAL_HTTP_PORT global involved — the leader_hint tests
// above rely on that global being unset for the whole test binary (no test
// calls set_local_http_port, so fallback is deterministic); these test the
// actual rewrite logic in isolation instead.

#[test]
fn test_replace_port_keeps_scheme_prefix() {
    assert_eq!(replace_port("http://node3:9081", 8080), "http://node3:8080");
}

#[test]
fn test_replace_port_no_scheme() {
    assert_eq!(replace_port("10.0.0.2:9081", 8080), "10.0.0.2:8080");
}

#[test]
fn test_replace_port_no_colon_appends_port() {
    // No `:` to split on at all — treat the whole string as the host.
    assert_eq!(replace_port("localhost", 8080), "localhost:8080");
}

// ── retry_after_ms_to_secs ───────────────────────────────────────────────────

#[test]
fn test_retry_after_ms_to_secs_rounds_up() {
    assert_eq!(retry_after_ms_to_secs(Some(150)), Some(1));
    assert_eq!(retry_after_ms_to_secs(Some(1001)), Some(2));
    assert_eq!(retry_after_ms_to_secs(Some(1000)), Some(1)); // exact multiple, no over-round
}

#[test]
fn test_retry_after_ms_to_secs_zero_is_omitted() {
    // 0 would read as "retry immediately" to most HTTP clients — misleading
    // when the real signal is "we don't have a useful wait estimate".
    assert_eq!(retry_after_ms_to_secs(Some(0)), None);
}

#[test]
fn test_retry_after_ms_to_secs_none_is_omitted() {
    assert_eq!(retry_after_ms_to_secs(None), None);
}

// ── normalize_error_body ─────────────────────────────────────────────────────
//
// Simulates the exact bug this middleware fixes: an extractor rejection (like
// `ValidKey`'s `(StatusCode, &'static str)`) produces a plain-text 4xx that
// bypasses `HttpError` entirely. Routed through a real Router + the layer (not
// called directly) so this proves the wiring in `mod.rs`, not just the function.

fn test_router() -> axum::Router {
    use axum::routing::get;

    axum::Router::new()
        .route("/plain-text-400", get(|| async { (StatusCode::BAD_REQUEST, "key must not contain '/'") }))
        .route(
            "/already-json-400",
            get(|| async {
                (
                    StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({"error": "already structured"})),
                )
            }),
        )
        .route("/ok", get(|| async { "fine" }))
        .layer(axum::middleware::map_response(normalize_error_body))
}

async fn get_response(path: &str) -> axum::response::Response {
    use tower::ServiceExt;

    let req = axum::http::Request::builder()
        .uri(path)
        .body(axum::body::Body::empty())
        .unwrap();
    test_router().oneshot(req).await.unwrap()
}

#[tokio::test]
async fn test_plain_text_rejection_gets_wrapped_into_json() {
    let resp = get_response("/plain-text-400").await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        resp.headers().get(axum::http::header::CONTENT_TYPE).unwrap(),
        "application/json"
    );
    let body = body_json(resp).await;
    assert_eq!(body["error"], "key must not contain '/'");
}

#[tokio::test]
async fn test_already_json_response_passes_through_unchanged() {
    let resp = get_response("/already-json-400").await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp).await;
    assert_eq!(body["error"], "already structured");
}

#[tokio::test]
async fn test_success_response_is_not_touched() {
    let resp = get_response("/ok").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&bytes[..], b"fine");
}

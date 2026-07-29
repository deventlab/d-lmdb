use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::Request;
use axum::http::StatusCode;
use d_lmdb::DLmdb;
use tempfile::TempDir;
use tower::ServiceExt;

use super::ConsistencyLevel;
use super::ConsistencyParam;
use super::route;

// ── ConsistencyLevel query parameter parsing ─────────────────────────────────

fn parse(query: &str) -> Result<ConsistencyParam, serde_urlencoded::de::Error> {
    serde_urlencoded::from_str(query)
}

#[test]
fn test_absent_level_defaults_to_eventual() {
    assert_eq!(parse("").unwrap().level, ConsistencyLevel::Eventual);
}

#[test]
fn test_level_eventual() {
    assert_eq!(
        parse("level=eventual").unwrap().level,
        ConsistencyLevel::Eventual
    );
}

#[test]
fn test_level_linearizable() {
    assert_eq!(
        parse("level=linearizable").unwrap().level,
        ConsistencyLevel::Linearizable
    );
}

#[test]
fn test_level_lease() {
    assert_eq!(parse("level=lease").unwrap().level, ConsistencyLevel::Lease);
}

#[test]
fn test_unknown_level_is_rejected() {
    assert!(parse("level=bogus").is_err());
}

// ── Handler tests (real DLmdb in temp dir) ───────────────────────────────────

/// Single-node DLmdb + router, ready for test requests.
async fn setup() -> (TempDir, Router) {
    let tmp = TempDir::new().expect("temp dir");
    let db = DLmdb::open(tmp.path()).await.expect("open db");
    db.wait_ready(std::time::Duration::from_secs(5)).await.expect("ready");
    let app = Router::new().route("/kv/{key}", route()).with_state(Arc::new(db));
    (tmp, app)
}

async fn put(
    app: &Router,
    key: &str,
    value: &str,
) -> StatusCode {
    let req = Request::builder()
        .method("PUT")
        .uri(format!("/kv/{key}"))
        .body(Body::from(value.to_string()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

async fn get(
    app: &Router,
    key: &str,
    query: &str,
) -> (StatusCode, String) {
    let uri = if query.is_empty() {
        format!("/kv/{key}")
    } else {
        format!("/kv/{key}?{query}")
    };
    let req = Request::builder().method("GET").uri(&uri).body(Body::empty()).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body_bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    let body_str = String::from_utf8_lossy(&body_bytes).to_string();
    (status, body_str)
}

async fn delete(
    app: &Router,
    key: &str,
) -> StatusCode {
    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/kv/{key}"))
        .body(Body::empty())
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

// ── PUT ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_put_new_key_returns_204_no_content() {
    let (_tmp, app) = setup().await;
    assert_eq!(put(&app, "hello", "world").await, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn test_put_overwrites_existing_key_and_returns_204() {
    let (_tmp, app) = setup().await;
    put(&app, "key", "v1").await;
    assert_eq!(put(&app, "key", "v2").await, StatusCode::NO_CONTENT);
    let (status, body) = get(&app, "key", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "v2");
}

// ── GET ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_get_returns_value_for_existing_key() {
    let (_tmp, app) = setup().await;
    put(&app, "greeting", "hello").await;
    let (status, body) = get(&app, "greeting", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "hello");
}

#[tokio::test]
async fn test_get_missing_key_returns_404() {
    let (_tmp, app) = setup().await;
    let (status, _body) = get(&app, "nonexistent", "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_get_linearizable_returns_value() {
    let (_tmp, app) = setup().await;
    put(&app, "linear-key", "val").await;
    let (status, body) = get(&app, "linear-key", "level=linearizable").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "val");
}

#[tokio::test]
async fn test_get_lease_returns_value() {
    let (_tmp, app) = setup().await;
    put(&app, "lease-key", "val").await;
    let (status, body) = get(&app, "lease-key", "level=lease").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "val");
}

#[tokio::test]
async fn test_get_returns_empty_value_for_empty_body_put() {
    let (_tmp, app) = setup().await;
    put(&app, "empty-value", "").await;
    let (status, body) = get(&app, "empty-value", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "");
}

// ── DELETE ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_delete_existing_key_returns_204() {
    let (_tmp, app) = setup().await;
    put(&app, "del-key", "x").await;
    assert_eq!(delete(&app, "del-key").await, StatusCode::NO_CONTENT);
    // Verify it's actually gone.
    assert_eq!(get(&app, "del-key", "").await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_delete_missing_key_returns_204_idempotent() {
    let (_tmp, app) = setup().await;
    assert_eq!(delete(&app, "never-existed").await, StatusCode::NO_CONTENT);
}

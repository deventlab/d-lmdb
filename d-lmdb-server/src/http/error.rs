//! Maps `d_lmdb::Error` to an HTTP response.
//!
//! | Status | Trigger |
//! |---|---|
//! | 400 | `KeyTooLarge` / `ValueTooLarge` / `Business{InvalidRequest}` |
//! | 503 | `Network{NotLeader}` (+ `leader_hint`, `Retry-After`) / `Business{ClusterUnavailable\|ProposeFailed\|RetryRequired\|TermOutdated}` |
//! | 500 | everything else — generic message by responsibility layer + `trace_id` |
//!
//! 409/429 are designed but **not implemented**: `StaleOperation`/`RateLimited` have
//! no real construction path through `EmbeddedEngine` today (see decision 015's Known
//! Limitation), so they fall through the wildcard into 500 rather than getting dead
//! match arms.

use std::sync::OnceLock;

use axum::Json;
use axum::http::HeaderValue;
use axum::http::StatusCode;
use axum::http::header::RETRY_AFTER;
use axum::response::IntoResponse;
use axum::response::Response;
use d_lmdb::ClientApiError;
use d_lmdb::ErrorCode;
use serde::Serialize;

#[derive(Serialize)]
struct ErrorBody {
    error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    leader_hint: Option<LeaderHintDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    trace_id: Option<String>,
}

/// `d_lmdb::LeaderHint.address` is the leader's Raft peer address (e.g.
/// `http://node3:9081`) — useless to an HTTP client, which needs the leader's
/// HTTP port instead. `address` here has the host from the Raft address with
/// this node's own HTTP port substituted in (see `rewrite_to_http_address`),
/// not `h.address` relayed verbatim.
#[derive(Serialize)]
struct LeaderHintDto {
    leader_id: u32,
    address: String,
}

/// This node's own `[http] listen_address` port, set once in `http::serve()`.
/// Read when rewriting a peer's Raft address into an HTTP one — see
/// `rewrite_to_http_address`.
static LOCAL_HTTP_PORT: OnceLock<u16> = OnceLock::new();

pub(super) fn set_local_http_port(port: u16) {
    let _ = LOCAL_HTTP_PORT.set(port);
}

/// Swaps the Raft port in a peer address for this node's own HTTP port.
/// Assumes every node in the cluster serves HTTP on the same port (true for
/// homogeneous deployments — same image/config, different hostnames — which
/// is what docker-compose/k8s give you). Falls back to the raw Raft address,
/// unusable as it is, only if this node's own HTTP port was somehow never set.
///
/// Known limitation: the rewritten address is only reachable by clients on the
/// same network as the cluster (e.g. other containers on the same docker-compose
/// network) — an external client reaching the cluster through published/mapped
/// ports won't be able to resolve the bare hostname either way. Not the primary
/// routing path regardless (external LB health-checking `/primary` is — see
/// `ha-deployment-load-balancing.md`); this only matters for direct-to-node
/// callers that skip the LB. See decisions doc `leader-hint-http-address-mismatch.md`.
fn rewrite_to_http_address(raft_address: &str) -> String {
    match LOCAL_HTTP_PORT.get() {
        Some(port) => replace_port(raft_address, *port),
        None => raft_address.to_string(),
    }
}

/// Pure string rewrite: replaces whatever comes after the last `:` with `port`.
/// Keeps any scheme prefix intact (`http://node3:9081` -> `http://node3:8080`).
fn replace_port(
    address: &str,
    port: u16,
) -> String {
    let host = address.rsplit_once(':').map_or(address, |(host, _)| host);
    format!("{host}:{port}")
}

pub(super) struct HttpError {
    status: StatusCode,
    body: ErrorBody,
    retry_after_secs: Option<u64>,
}

impl HttpError {
    pub(super) fn not_found() -> Self {
        Self::plain(StatusCode::NOT_FOUND, "key not found")
    }

    fn plain(
        status: StatusCode,
        message: &str,
    ) -> Self {
        Self {
            status,
            body: ErrorBody {
                error: message.to_string(),
                leader_hint: None,
                trace_id: None,
            },
            retry_after_secs: None,
        }
    }
}

impl From<d_lmdb::Error> for HttpError {
    fn from(err: d_lmdb::Error) -> Self {
        match &err {
            d_lmdb::Error::KeyTooLarge(_) | d_lmdb::Error::ValueTooLarge(_) => {
                Self::plain(StatusCode::BAD_REQUEST, &err.to_string())
            }

            d_lmdb::Error::Client(ClientApiError::Business {
                code: ErrorCode::InvalidRequest,
                message,
                ..
            }) => Self::plain(StatusCode::BAD_REQUEST, message),

            d_lmdb::Error::Client(ClientApiError::Network {
                code: ErrorCode::NotLeader,
                message,
                retry_after_ms,
                leader_hint,
            }) => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                body: ErrorBody {
                    error: message.clone(),
                    leader_hint: leader_hint.as_ref().map(|h| LeaderHintDto {
                        leader_id: h.leader_id,
                        address: rewrite_to_http_address(&h.address),
                    }),
                    trace_id: None,
                },
                retry_after_secs: retry_after_ms_to_secs(*retry_after_ms),
            },

            d_lmdb::Error::Client(ClientApiError::Business {
                code:
                    ErrorCode::ClusterUnavailable
                    | ErrorCode::ProposeFailed
                    | ErrorCode::RetryRequired
                    | ErrorCode::TermOutdated,
                message,
                ..
            }) => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                body: ErrorBody {
                    error: message.clone(),
                    leader_hint: None,
                    trace_id: None,
                },
                retry_after_secs: None,
            },

            _ => {
                let trace_id = uuid::Uuid::new_v4().to_string();
                tracing::error!(trace_id = %trace_id, error = %err, "unhandled d_lmdb error");
                Self {
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                    body: ErrorBody {
                        error: internal_error_category(&err).to_string(),
                        leader_hint: None,
                        trace_id: Some(trace_id),
                    },
                    retry_after_secs: None,
                }
            }
        }
    }
}

/// 500 message grouped by responsibility layer — never a path, stack trace, or raw
/// error string. Exact detail lives in the `trace_id`-tagged log line, not the body.
fn internal_error_category(err: &d_lmdb::Error) -> &'static str {
    match err {
        d_lmdb::Error::Lmdb(_) | d_lmdb::Error::Storage(_) | d_lmdb::Error::Io(_) => {
            "storage error"
        }
        d_lmdb::Error::Engine(_) => "cluster error",
        d_lmdb::Error::Client(ClientApiError::Business {
            code: ErrorCode::Uncategorized,
            ..
        }) => "cluster error",
        d_lmdb::Error::ConfigError(_) | d_lmdb::Error::Path(_) => "configuration error",
        _ => "internal error",
    }
}

/// `Retry-After` is in seconds (RFC 9110); `retry_after_ms` is milliseconds. Rounds
/// up: the header means "wait at least this long", and a bare `0` reads as "retry
/// immediately" to most HTTP clients — wrong signal for a 503.
fn retry_after_ms_to_secs(ms: Option<u64>) -> Option<u64> {
    match ms {
        Some(0) | None => None,
        Some(ms) => Some(ms.div_ceil(1000)),
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let mut response = (self.status, Json(self.body)).into_response();
        if let Some(secs) = self.retry_after_secs
            && let Ok(value) = HeaderValue::from_str(&secs.to_string())
        {
            response.headers_mut().insert(RETRY_AFTER, value);
        }
        response
    }
}

/// Wraps any non-JSON 4xx/5xx response (extractor rejections — `ValidKey`, axum's
/// own `QueryRejection`, etc.) into the same `{"error": ...}` shape `HttpError`
/// produces, so callers can always parse the body as JSON regardless of which
/// layer generated the error. The original text is preserved verbatim as `error`
/// — this normalizes the envelope, not the wording.
pub(super) async fn normalize_error_body(response: Response) -> Response {
    if !response.status().is_client_error() && !response.status().is_server_error() {
        return response;
    }
    let is_json = response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    if is_json {
        return response;
    }

    let status = response.status();
    let message = match axum::body::to_bytes(response.into_body(), usize::MAX).await {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(_) => "internal error".to_string(),
    };
    (
        status,
        Json(ErrorBody {
            error: message,
            leader_hint: None,
            trace_id: None,
        }),
    )
        .into_response()
}

#[cfg(test)]
#[path = "error_test.rs"]
mod error_test;

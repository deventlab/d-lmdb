//! PUT/GET/DELETE /kv/:key — key validation, body limit, wraps DLmdb put/get/delete.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::FromRequestParts;
use axum::extract::Path;
use axum::extract::Query;
use axum::extract::State;
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::routing::MethodRouter;
use axum::routing::get;
use d_lmdb::DLmdb;
use serde::Deserialize;

use super::error::HttpError;

/// A `/kv/:key` path segment that has already passed validation: UTF-8 (guaranteed by
/// axum's `Path<String>`), non-empty, and does not contain `/` (v1 scope — see plan).
struct ValidKey(String);

impl<S> FromRequestParts<S> for ValidKey
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        let Path(key) = Path::<String>::from_request_parts(parts, state)
            .await
            .map_err(|_| (StatusCode::BAD_REQUEST, "invalid key encoding"))?;
        if key.is_empty() {
            return Err((StatusCode::BAD_REQUEST, "key must not be empty"));
        }
        if key.contains('/') {
            return Err((StatusCode::BAD_REQUEST, "key must not contain '/'"));
        }
        Ok(ValidKey(key))
    }
}

/// `?level=` picks how fresh a read must be, trading speed for certainty.
/// Absent or `eventual` (default): fast local read, may be briefly stale.
/// `linearizable`: confirmed via Raft ReadIndex — always current, slower.
/// `lease`: leader-lease optimized — as current as `linearizable`, faster
/// when the lease is valid; must land on the leader.
#[derive(Deserialize, Default, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
enum ConsistencyLevel {
    #[default]
    Eventual,
    Linearizable,
    Lease,
}

#[derive(Deserialize)]
struct ConsistencyParam {
    #[serde(default)]
    level: ConsistencyLevel,
}

async fn get_handler(
    State(db): State<Arc<DLmdb>>,
    ValidKey(key): ValidKey,
    Query(q): Query<ConsistencyParam>,
) -> Result<Bytes, HttpError> {
    let value = match q.level {
        ConsistencyLevel::Eventual => db.get(&key)?,
        ConsistencyLevel::Linearizable => db.get_linearizable(&key).await?,
        ConsistencyLevel::Lease => db.get_lease(&key).await?,
    };
    match value {
        Some(bytes) => Ok(bytes),
        None => Err(HttpError::not_found()),
    }
}

async fn put_handler(
    State(db): State<Arc<DLmdb>>,
    ValidKey(key): ValidKey,
    body: Bytes,
) -> Result<StatusCode, HttpError> {
    db.put(&key, &body).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_handler(
    State(db): State<Arc<DLmdb>>,
    ValidKey(key): ValidKey,
) -> Result<StatusCode, HttpError> {
    db.delete(&key).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Wires all three /kv/:key methods onto one route.
pub(super) fn route() -> MethodRouter<Arc<DLmdb>> {
    get(get_handler).put(put_handler).delete(delete_handler)
}

#[cfg(test)]
#[path = "kv_test.rs"]
mod kv_test;

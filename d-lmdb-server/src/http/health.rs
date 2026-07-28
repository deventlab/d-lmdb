//! /primary, /replica, /status — read-only endpoints for LB health checks and operators.

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use d_lmdb::DLmdb;
use d_lmdb::LeaderInfo;
use serde::Serialize;

/// LB target for write routing: 200 if this node is leader, else 503.
async fn primary_handler(State(db): State<Arc<DLmdb>>) -> StatusCode {
    if db.is_leader() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

/// LB target for read routing: 200 if this node is a follower, else 503.
async fn replica_handler(State(db): State<Arc<DLmdb>>) -> StatusCode {
    if db.is_leader() {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::OK
    }
}

/// Human-facing: current leader + cluster membership. Not used for LB routing.
#[derive(Serialize)]
struct StatusResponse {
    leader: Option<LeaderInfo>,
    members: Vec<u32>,
    learners: Vec<u32>,
    committed_index: u64,
}

async fn status_handler(State(db): State<Arc<DLmdb>>) -> Json<StatusResponse> {
    let membership = db.cluster_info();
    Json(StatusResponse {
        leader: db.leader(),
        members: membership.members.into_iter().collect(),
        learners: membership.learners.into_iter().collect(),
        committed_index: membership.committed_index,
    })
}

/// Merges /primary, /replica, /status onto the given router.
pub(super) fn add_routes(router: Router<Arc<DLmdb>>) -> Router<Arc<DLmdb>> {
    router
        .route("/primary", get(primary_handler))
        .route("/replica", get(replica_handler))
        .route("/status", get(status_handler))
}

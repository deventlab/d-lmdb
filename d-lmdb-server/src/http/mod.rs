//! HTTP entry point: router assembly, shared `DLmdb` state, `serve()` for main.rs.

mod error;
mod health;
mod kv;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use d_lmdb::DLmdb;
use tower_http::limit::RequestBodyLimitLayer;

/// JSON/transport overhead allowance on top of DLmdb's configured max value size,
/// so a value that's exactly at the limit doesn't get rejected by the transport
/// layer before it ever reaches `db.put()`'s own validation.
const BODY_LIMIT_OVERHEAD_BYTES: usize = 1024;

/// Builds the router and serves it on `addr` until the process exits.
pub async fn serve(
    db: Arc<DLmdb>,
    addr: SocketAddr,
) -> std::io::Result<()> {
    let body_limit = db.max_value_bytes() + BODY_LIMIT_OVERHEAD_BYTES;
    error::set_local_http_port(addr.port());

    let app = Router::new().route("/kv/{key}", kv::route());
    let app = health::add_routes(app);
    let app = app
        .layer(RequestBodyLimitLayer::new(body_limit))
        .layer(axum::middleware::map_response(error::normalize_error_body))
        .with_state(db);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await
}

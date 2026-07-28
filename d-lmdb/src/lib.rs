//! d-lmdb wraps LMDB with Raft consensus (via [d-engine](https://github.com/DEventLab/d-engine)).
//! Reads stay local and synchronous; writes go through Raft and become replicated and
//! strongly consistent.
//!
//! d-lmdb targets distributed **fault tolerance**, not distributed **scaling**: every
//! node holds the full dataset (no sharding), and write throughput is traded for
//! availability and consistency. See the [README](https://github.com/DEventLab/d-lmdb)
//! for what this does and does not solve.
//!
//! # Quick start
//!
//! ```rust
//! use d_lmdb::DLmdb;
//! use std::time::Duration;
//!
//! #[tokio::main]
//! async fn main() -> d_lmdb::Result<()> {
//!     let db = DLmdb::open("./data").await?;
//!     db.wait_ready(Duration::from_secs(5)).await?;
//!
//!     db.put(b"user:1", b"alice").await?;
//!     let val = db.get(b"user:1")?; // sync, no await — bypasses Raft
//!     Ok(())
//! }
//! ```
//!
//! See `examples/single-node` and `examples/three-node` for full runnable setups.

#![warn(missing_docs)]

mod config;
mod db;
mod error;
mod state_machine;
mod storage_engine;
mod time;
mod wire;

pub use d_engine::BatchOp;
pub use d_engine::ClientApiError;
pub use d_engine::ErrorCode;
pub use d_engine::LeaderHint;
pub use d_engine::LeaderInfo;
pub use db::DLmdb;
pub use error::Error;
pub use error::Result;
pub(crate) use time::*;

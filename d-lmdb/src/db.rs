use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use d_engine::BatchOp;
use d_engine::ClientApi;
use d_engine::EmbeddedEngine;
use d_engine::LeaderInfo;
use d_engine::MembershipSnapshot;
use d_engine::RaftNodeConfig;
use d_engine::ScanResult;
use d_engine::StateMachine;

use crate::Error;
use crate::Result;
use crate::config::DLmdbConfig;
use crate::state_machine::LmdbStateMachine;
use crate::storage_engine::LmdbStorageEngine;
use crate::unix_now_secs;
use crate::wire::Value;

type Inner = EmbeddedEngine<LmdbStorageEngine, LmdbStateMachine>;

/// A distributed key-value database.
///
/// d-engine (Raft) handles write consensus; reads bypass Raft and go directly
/// to the local LMDB instance. No external servers, no sidecars.
///
/// ```rust,ignore
/// let db = DLmdb::open("./data").await?;
/// db.wait_ready(Duration::from_secs(5)).await?;
///
/// db.put(b"user:1", b"alice").await?;
/// let val = db.get(b"user:1")?;  // sync, no await
/// ```
pub struct DLmdb {
    inner: Inner,
    state_machine: Arc<LmdbStateMachine>,
    max_key_bytes: usize,
    max_value_bytes: usize,
}

impl DLmdb {
    /// Open (or create) a d-lmdb node.
    pub async fn open_from_file(config_path: impl AsRef<Path>) -> Result<Self> {
        let path_str = config_path
            .as_ref()
            .to_str()
            .ok_or_else(|| Error::Path("path is not valid UTF-8".into()))?;

        let dlmdb_config = DLmdbConfig::from_file(path_str)?;

        let raft_dir = dlmdb_config.data_dir.join("raft");
        let lmdb_dir = dlmdb_config.data_dir.join("lmdb");

        let storage_engine = Arc::new(LmdbStorageEngine::new(raft_dir)?);
        let state_machine = Arc::new(LmdbStateMachine::new(lmdb_dir, &dlmdb_config).await?);
        let sm_ref = Arc::clone(&state_machine);

        let engine = EmbeddedEngine::<LmdbStorageEngine, LmdbStateMachine>::start_custom(
            storage_engine,
            state_machine,
            Some(path_str),
        )
        .await?;

        Ok(Self {
            inner: engine,
            state_machine: sm_ref,
            max_key_bytes: dlmdb_config.max_key_bytes,
            max_value_bytes: dlmdb_config.max_value_bytes,
        })
    }
    /// Open (or create) a d-lmdb node.
    pub async fn open(data_path: impl AsRef<Path>) -> Result<Self> {
        let path_str = data_path.as_ref();
        let raft_dir = path_str.join("raft");
        let lmdb_dir = path_str.join("lmdb");

        let dlmdb_config = DLmdbConfig::new(data_path);

        let storage_engine = Arc::new(LmdbStorageEngine::new(raft_dir.clone())?);
        let state_machine = Arc::new(LmdbStateMachine::new(lmdb_dir, &dlmdb_config).await?);
        let sm_ref = Arc::clone(&state_machine);

        let mut raft_config = RaftNodeConfig::new()?;
        raft_config.cluster.db_root_dir = raft_dir;

        let engine = EmbeddedEngine::<LmdbStorageEngine, LmdbStateMachine>::start_node(
            raft_config,
            storage_engine,
            state_machine,
        )
        .await?;

        Ok(Self {
            inner: engine,
            state_machine: sm_ref,
            max_key_bytes: dlmdb_config.max_key_bytes,
            max_value_bytes: dlmdb_config.max_value_bytes,
        })
    }

    /// Block until this node has elected a leader and is ready to serve writes.
    pub async fn wait_ready(
        &self,
        timeout: Duration,
    ) -> Result<()> {
        self.inner.wait_ready(timeout).await?;
        Ok(())
    }

    // ---- sync reads: bypass Raft, direct LMDB memory-map -------------------

    /// Eventual read. Not guaranteed to reflect the latest committed write.
    pub fn get(
        &self,
        key: impl AsRef<[u8]>,
    ) -> Result<Option<Bytes>> {
        self.get_live(key.as_ref())
    }

    /// Eventual read. Not guaranteed to reflect the latest committed write.
    pub fn exists(
        &self,
        key: impl AsRef<[u8]>,
    ) -> Result<bool> {
        Ok(self.get_live(key.as_ref())?.is_some())
    }

    /// Eventual read. Not guaranteed to reflect the latest committed write.
    pub fn get_multi<K: AsRef<[u8]>>(
        &self,
        keys: &[K],
    ) -> Result<Vec<Option<Bytes>>> {
        keys.iter().map(|k| self.get_live(k.as_ref())).collect()
    }

    /// Reads the raw value from the state machine (byte-transparent) and applies
    /// TTL semantics on top: decode the wire envelope, treat an expired TTL value
    /// as absent, and physically clean up expired entries synchronously
    /// (local-only — see `LmdbStateMachine::delete_local`; safe without Raft
    /// because expiry is decided by the already-replicated `expires_at`).
    fn get_live(
        &self,
        key: &[u8],
    ) -> Result<Option<Bytes>> {
        let Some(raw) = self.state_machine.get(key)? else {
            return Ok(None);
        };
        match Value::decode(&raw) {
            Ok(Value::Raw(payload)) => Ok(Some(Bytes::copy_from_slice(payload))),
            Ok(Value::Ttl {
                expires_at,
                payload,
            }) => {
                if expires_at <= unix_now_secs() {
                    self.state_machine.delete_local(key)?;
                    Ok(None)
                } else {
                    Ok(Some(Bytes::copy_from_slice(payload)))
                }
            }
            Err(_) => Err(Error::Storage("corrupt value: invalid tag byte".into())),
        }
    }

    /// Number of keys. Eventual read — may lag behind the leader.
    pub fn len(&self) -> usize {
        self.state_machine.len()
    }

    /// Eventual read — may lag behind the leader.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Eventual read. Not guaranteed to reflect the latest committed write.
    ///
    /// Bounded scan: returns at most `limit` entries. If `entries.len() == limit`,
    /// the result may be truncated. Pass `None` to scan without a cap (may load
    /// large datasets into memory).
    pub fn scan_all(
        &self,
        limit: Option<usize>,
    ) -> Result<ScanResult> {
        Ok(self.state_machine.scan_all(limit, Some(ttl_filter))?)
    }

    /// Eventual read. Not guaranteed to reflect the latest committed write.
    ///
    /// Bounded scan: returns at most `limit` entries. If `entries.len() == limit`,
    /// the result may be truncated. `after` is an exclusive lower bound for
    /// pagination — pass the last key from the previous page to continue scanning.
    pub fn scan_prefix(
        &self,
        prefix: impl AsRef<[u8]>,
        after: Option<&[u8]>,
        limit: Option<usize>,
    ) -> Result<ScanResult> {
        Ok(self.state_machine.scan_prefix_bounded(
            prefix.as_ref(),
            after,
            limit,
            Some(ttl_filter),
        )?)
    }

    /// Eventual read. Not guaranteed to reflect the latest committed write.
    ///
    /// Scans keys in `[start, end)`, sorted ascending. Bounded scan: returns at
    /// most `limit` entries. If `entries.len() == limit`, the result may be truncated.
    pub fn scan_range(
        &self,
        start: impl AsRef<[u8]>,
        end: impl AsRef<[u8]>,
        limit: Option<usize>,
    ) -> Result<ScanResult> {
        Ok(self.state_machine.scan_range(
            start.as_ref(),
            Some(end.as_ref()),
            limit,
            Some(ttl_filter),
        )?)
    }

    /// Eventual read. Not guaranteed to reflect the latest committed write.
    ///
    /// Scans keys in `[start, end)` in descending order. Bounded scan: returns at
    /// most `limit` entries. If `entries.len() == limit`, the result may be truncated.
    pub fn scan_range_rev(
        &self,
        start: impl AsRef<[u8]>,
        end: impl AsRef<[u8]>,
        limit: Option<usize>,
    ) -> Result<ScanResult> {
        Ok(self.state_machine.scan_range_rev(
            start.as_ref(),
            end.as_ref(),
            limit,
            Some(ttl_filter),
        )?)
    }

    // ---- async linearizable read: Raft ReadIndex round-trip ----------------

    /// Linearizable read. Reflects all writes committed before this call.
    pub async fn get_linearizable(
        &self,
        key: impl AsRef<[u8]>,
    ) -> Result<Option<Bytes>> {
        Ok(self.inner.client().get_linearizable(key).await?)
    }

    // ---- async writes: Raft consensus ---------------------------------------

    /// Write a key-value pair. Strongly consistent (goes through Raft leader).
    pub async fn put(
        &self,
        key: impl AsRef<[u8]>,
        value: impl AsRef<[u8]>,
    ) -> Result<()> {
        validate_key(key.as_ref(), self.max_key_bytes)?;
        validate_value(value.as_ref(), self.max_value_bytes)?;

        self.inner.client().put(key, Value::encode_raw(value.as_ref())).await?;
        Ok(())
    }

    /// Write `key`/`value` with an expiry. The key is physically removed once
    /// `ttl_secs` have elapsed, on the next read/scan that observes it.
    pub async fn put_with_ttl(
        &self,
        key: impl AsRef<[u8]>,
        value: impl AsRef<[u8]>,
        ttl_secs: u64,
    ) -> Result<()> {
        validate_key(key.as_ref(), self.max_key_bytes)?;
        validate_value(value.as_ref(), self.max_value_bytes)?;

        let expires_at = unix_now_secs() + ttl_secs;
        self.inner
            .client()
            .put(key, Value::encode_ttl(value.as_ref(), Some(expires_at)))
            .await?;
        Ok(())
    }

    /// Apply a batch of inserts/deletes atomically through Raft.
    pub async fn batch(
        &self,
        ops: Vec<BatchOp>,
    ) -> Result<()> {
        let encoded: Vec<BatchOp> = ops
            .into_iter()
            .map(|op| match op {
                BatchOp::Insert { key, value } => BatchOp::Insert {
                    key,
                    value: Value::encode_raw(&value).into(),
                },
                BatchOp::Delete { key } => BatchOp::Delete { key },
            })
            .collect();
        self.inner.client().batch(encoded).await?;
        Ok(())
    }

    /// Delete a key.
    pub async fn delete(
        &self,
        key: impl AsRef<[u8]>,
    ) -> Result<()> {
        self.inner.client().delete(key).await?;
        Ok(())
    }

    /// Insert `key` only if it does not already exist. Returns `true` if inserted.
    pub async fn put_if_absent(
        &self,
        key: impl AsRef<[u8]> + Send,
        value: impl AsRef<[u8]> + Send,
    ) -> Result<bool> {
        Ok(self
            .inner
            .client()
            .compare_and_swap(key, None::<&[u8]>, Value::encode_raw(value.as_ref()))
            .await?)
    }

    /// Atomic compare-and-swap. Returns `true` if the swap was applied.
    pub async fn compare_and_swap(
        &self,
        key: impl AsRef<[u8]> + Send,
        expected: Option<impl AsRef<[u8]> + Send>,
        new_value: impl AsRef<[u8]> + Send,
    ) -> Result<bool> {
        let enc_expected = expected.map(|e| Value::encode_raw(e.as_ref()));
        let enc_new = Value::encode_raw(new_value.as_ref());
        Ok(self
            .inner
            .client()
            .compare_and_swap(key, enc_expected.as_deref(), enc_new)
            .await?)
    }

    /// Returns the current leader, or `None` if no leader is elected yet.
    pub fn leader(&self) -> Option<LeaderInfo> {
        self.inner.leader_info()
    }

    /// Check if current node is leader or not.
    pub fn is_leader(&self) -> bool {
        self.inner.is_leader()
    }

    /// Returns a snapshot of the current cluster membership.
    pub fn cluster_info(&self) -> MembershipSnapshot {
        self.inner.watch_membership().borrow().clone()
    }

    /// Gracefully stop the node.
    pub async fn close(self) -> Result<()> {
        self.inner.stop().await?;
        Ok(())
    }
}

// ---- scan filter --------------------------------------------------------------

/// Decides whether a raw `kv_db` value is live, and what to return for it.
/// `LmdbStateMachine` treats this as an opaque function — it has no idea what
/// a "tag byte" or "expires_at" is. `None` drops the entry (expired, or an
/// unrecognized/corrupt tag); `Some(payload)` keeps it, already stripped of
/// the wire envelope.
fn ttl_filter(raw: &[u8]) -> Option<Vec<u8>> {
    match Value::decode(raw) {
        Ok(Value::Raw(payload)) => Some(payload.to_vec()),
        Ok(Value::Ttl {
            expires_at,
            payload,
        }) if expires_at > unix_now_secs() => Some(payload.to_vec()),
        _ => None,
    }
}

// ---- guards -----------------------------------------------------------------

fn validate_key(
    key: &[u8],
    max: usize,
) -> Result<()> {
    if key.len() > max {
        return Err(Error::KeyTooLarge(key.len()));
    }
    Ok(())
}

fn validate_value(
    value: &[u8],
    max: usize,
) -> Result<()> {
    if value.len() > max {
        return Err(Error::ValueTooLarge(value.len()));
    }
    Ok(())
}

#[cfg(test)]
#[path = "db_test.rs"]
mod db_test;

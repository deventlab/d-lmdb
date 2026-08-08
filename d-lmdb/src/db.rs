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
use crate::lock::DataDirLock;
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
    _data_dir_lock: DataDirLock,
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

        let data_dir_lock = DataDirLock::acquire(&dlmdb_config.data_dir)?;

        let raft_dir = dlmdb_config.data_dir.join("raft");
        let lmdb_dir = dlmdb_config.data_dir.join("lmdb");

        let storage_engine = Arc::new(LmdbStorageEngine::new(raft_dir)?);
        let state_machine = Arc::new(LmdbStateMachine::new(lmdb_dir, &dlmdb_config).await?);
        let sm_ref = Arc::clone(&state_machine);

        let engine = EmbeddedEngine::<LmdbStorageEngine, LmdbStateMachine>::start_custom(
            &dlmdb_config.data_dir,
            storage_engine,
            state_machine,
            Some(path_str),
        )
        .await?;

        Ok(Self {
            inner: engine,
            state_machine: sm_ref,
            _data_dir_lock: data_dir_lock,
            max_key_bytes: dlmdb_config.max_key_bytes,
            max_value_bytes: dlmdb_config.max_value_bytes,
        })
    }
    /// Open (or create) a d-lmdb node.
    pub async fn open(data_path: impl AsRef<Path>) -> Result<Self> {
        let path_str = data_path.as_ref();
        let raft_dir = path_str.join("raft");
        let lmdb_dir = path_str.join("lmdb");

        let dlmdb_config = DLmdbConfig::new(path_str);

        let data_dir_lock = DataDirLock::acquire(path_str)?;

        let storage_engine = Arc::new(LmdbStorageEngine::new(raft_dir)?);
        let state_machine = Arc::new(LmdbStateMachine::new(lmdb_dir, &dlmdb_config).await?);
        let sm_ref = Arc::clone(&state_machine);

        let raft_config = RaftNodeConfig::new()?;

        let engine = EmbeddedEngine::<LmdbStorageEngine, LmdbStateMachine>::start_node(
            path_str,
            storage_engine,
            state_machine,
            raft_config,
        )
        .await?;

        Ok(Self {
            inner: engine,
            state_machine: sm_ref,
            _data_dir_lock: data_dir_lock,
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
        self.fetch_local(key.as_ref())
    }

    /// Eventual read. Not guaranteed to reflect the latest committed write.
    pub fn exists(
        &self,
        key: impl AsRef<[u8]>,
    ) -> Result<bool> {
        Ok(self.fetch_local(key.as_ref())?.is_some())
    }

    /// Eventual read. Not guaranteed to reflect the latest committed write.
    pub fn get_multi<K: AsRef<[u8]>>(
        &self,
        keys: &[K],
    ) -> Result<Vec<Option<Bytes>>> {
        keys.iter().map(|k| self.fetch_local(k.as_ref())).collect()
    }

    /// Raw bytes from the local state machine, then `decode_and_reap`.
    fn fetch_local(
        &self,
        key: &[u8],
    ) -> Result<Option<Bytes>> {
        let raw = self.state_machine.get(key)?;
        self.decode_and_reap(key, raw)
    }

    /// Shared by all read paths: decode + apply TTL. Local delete-on-expiry is
    /// safe without Raft since `expires_at` is already replicated.
    fn decode_and_reap(
        &self,
        key: &[u8],
        raw: Option<Bytes>,
    ) -> Result<Option<Bytes>> {
        let Some(raw) = raw else {
            return Ok(None);
        };
        match decode_live_value(&raw, unix_now_secs())? {
            DecodedValue::Present(payload) => Ok(Some(payload)),
            DecodedValue::Expired => {
                self.state_machine.delete_local(key)?;
                Ok(None)
            }
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
        Ok(self.state_machine.scan_all(limit, Some(decode_for_scan))?)
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
            Some(decode_for_scan),
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
            Some(decode_for_scan),
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
            Some(decode_for_scan),
        )?)
    }

    // ---- async linearizable read: Raft ReadIndex round-trip ----------------

    /// Linearizable read. Reflects all writes committed before this call.
    pub async fn get_linearizable(
        &self,
        key: impl AsRef<[u8]>,
    ) -> Result<Option<Bytes>> {
        let key = key.as_ref();
        let raw = self.inner.client().get_linearizable(key).await?;
        self.decode_and_reap(key, raw)
    }

    // ---- async lease read: leader-lease optimized, still strongly consistent ----

    /// Lease read. Same guarantee as `get_linearizable`, faster when the leader's
    /// lease is valid. Lease duration is a d-engine startup config, not d-lmdb's.
    pub async fn get_lease(
        &self,
        key: impl AsRef<[u8]>,
    ) -> Result<Option<Bytes>> {
        let key = key.as_ref();
        let raw = self.inner.client().get_lease(key).await?;
        self.decode_and_reap(key, raw)
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

    /// The configured max value size in bytes. Exposed so callers building a
    /// layer on top (e.g. an HTTP request body limit) can reuse this instead
    /// of duplicating it as a separate config value.
    pub fn max_value_bytes(&self) -> usize {
        self.max_value_bytes
    }

    /// Gracefully stop the node.
    pub async fn close(self) -> Result<()> {
        self.inner.stop().await?;
        Ok(())
    }
}

// ---- live-read decode -----------------------------------------------------------

/// Outcome of decoding a stored value against the current time.
#[derive(Debug, PartialEq, Eq)]
enum DecodedValue {
    /// Present and live: payload with the wire envelope stripped.
    Present(Bytes),
    /// Present in storage but past its TTL — logically absent.
    Expired,
}

/// Decodes a raw stored value and applies TTL semantics. Pure — no I/O.
///
/// Note: expired entries found via `decode_for_scan` (scan path) are NOT
/// physically reaped, unlike point reads via `decode_and_reap` — scans run
/// on a read-only LMDB txn and structurally cannot delete.
fn decode_live_value(
    raw: &[u8],
    now_secs: u64,
) -> Result<DecodedValue> {
    match Value::decode(raw) {
        Ok(Value::Raw(payload)) => Ok(DecodedValue::Present(Bytes::copy_from_slice(payload))),
        Ok(Value::Ttl {
            expires_at,
            payload,
        }) => {
            if expires_at <= now_secs {
                Ok(DecodedValue::Expired)
            } else {
                Ok(DecodedValue::Present(Bytes::copy_from_slice(payload)))
            }
        }
        Err(_) => Err(Error::Storage("corrupt value: invalid tag byte".into())),
    }
}

// ---- scan filter --------------------------------------------------------------

/// Decides whether a raw `kv_db` value is live, and what to return for it.
/// `LmdbStateMachine` treats this as an opaque function — it has no idea what
/// a "tag byte" or "expires_at" is. `None` drops the entry (expired, or an
/// unrecognized/corrupt tag); `Some(payload)` keeps it, already stripped of
/// the wire envelope.
fn decode_for_scan(raw: &[u8]) -> Option<Vec<u8>> {
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

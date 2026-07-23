use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::ops::Bound;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::Mutex;

use async_trait::async_trait;
use bytes::Bytes;
use d_engine::common::LogId;
use d_engine::server_storage::SnapshotMetadata;
use d_engine::ApplyEntry;
use d_engine::ApplyResult;
use d_engine::Command;
use d_engine::Error as EngineError;
use d_engine::ScanResult;
use d_engine::StateMachine;
use d_engine::StorageError;
use heed::types::Bytes as LmdbBytes;
use heed::CompactionOption;
use heed::Database;
use heed::Env;
use heed::EnvOpenOptions;
use tracing::info;

use crate::config::DLmdbConfig;
use crate::unix_now_secs;
use crate::wire::Value;

const DB_KV: &str = "kv";
const DB_META: &str = "meta";
const DB_INTERNAL: &str = "internal";

const META_LAST_APPLIED_INDEX: &str = "last_applied_index";
const META_LAST_APPLIED_TERM: &str = "last_applied_term";

#[derive(Debug)]
pub struct LmdbStateMachine {
    env: Env,
    kv_db: Database<LmdbBytes, LmdbBytes>,
    meta_db: Database<heed::types::Str, LmdbBytes>,
    internal_db: Database<LmdbBytes, LmdbBytes>,

    running: AtomicBool,
    last_applied_index: AtomicU64,
    last_applied_term: AtomicU64,

    // In-memory term cache: log_index → term. Populated during apply_chunk.
    // Not persisted across restarts (reconstructed from Raft log on recovery).
    entry_terms: Mutex<BTreeMap<u64, u64>>,

    snapshot_meta: Mutex<Option<SnapshotMetadata>>,
    map_size: usize,
}

impl LmdbStateMachine {
    pub async fn new(
        data_dir: PathBuf,
        config: &DLmdbConfig,
    ) -> crate::Result<Self> {
        tokio::fs::create_dir_all(&data_dir).await?;

        let env = unsafe {
            EnvOpenOptions::new().map_size(config.map_size_gb).max_dbs(3).open(&data_dir)?
        };

        let (kv_db, meta_db, internal_db, last_applied_index, last_applied_term) = {
            let mut wtxn = env.write_txn()?;
            let kv_db: Database<LmdbBytes, LmdbBytes> =
                env.create_database(&mut wtxn, Some(DB_KV))?;
            let meta_db: Database<heed::types::Str, LmdbBytes> =
                env.create_database(&mut wtxn, Some(DB_META))?;
            let internal_db: Database<LmdbBytes, LmdbBytes> =
                env.create_database(&mut wtxn, Some(DB_INTERNAL))?;

            let index =
                meta_db.get(&wtxn, META_LAST_APPLIED_INDEX)?.map(u64_from_bytes).unwrap_or(0);
            let term = meta_db.get(&wtxn, META_LAST_APPLIED_TERM)?.map(u64_from_bytes).unwrap_or(0);

            wtxn.commit()?;
            (kv_db, meta_db, internal_db, index, term)
        };

        info!(
            last_applied_index,
            last_applied_term,
            ?data_dir,
            "LmdbStateMachine opened"
        );

        Ok(Self {
            env,
            kv_db,
            meta_db,
            internal_db,
            running: AtomicBool::new(true),
            last_applied_index: AtomicU64::new(last_applied_index),
            last_applied_term: AtomicU64::new(last_applied_term),
            entry_terms: Mutex::new(BTreeMap::new()),
            snapshot_meta: Mutex::new(None),
            map_size: config.map_size_gb,
        })
    }
}

impl LmdbStateMachine {
    // All blocking LMDB I/O for snapshot restore runs on tokio's blocking thread pool.
    // Takes owned/copied handles so it can be moved into spawn_blocking without borrowing self.
    fn restore_blocking(
        env: Env,
        kv_db: Database<LmdbBytes, LmdbBytes>,
        meta_db: Database<heed::types::Str, LmdbBytes>,
        map_size: usize,
        metadata: SnapshotMetadata,
        snapshot_path: std::path::PathBuf,
    ) -> Result<(), EngineError> {
        let snap_env = unsafe {
            EnvOpenOptions::new()
                .map_size(map_size)
                .max_dbs(3)
                .open(&snapshot_path)
                .map_err(|e| EngineError::Fatal(e.to_string()))?
        };

        let snap_rtxn = snap_env.read_txn().map_err(|e| EngineError::Fatal(e.to_string()))?;
        let snap_kv: Database<LmdbBytes, LmdbBytes> = snap_env
            .open_database(&snap_rtxn, Some(DB_KV))
            .map_err(|e| EngineError::Fatal(e.to_string()))?
            .ok_or_else(|| EngineError::Fatal("snapshot missing kv database".into()))?;

        // Clear current KV and copy from snapshot in a single write txn.
        let mut wtxn = env.write_txn().map_err(|e| EngineError::Fatal(e.to_string()))?;
        kv_db.clear(&mut wtxn).map_err(|e| EngineError::Fatal(e.to_string()))?;

        for item in snap_kv.iter(&snap_rtxn).map_err(|e| EngineError::Fatal(e.to_string()))? {
            let (k, v) = item.map_err(|e| EngineError::Fatal(e.to_string()))?;
            kv_db.put(&mut wtxn, k, v).map_err(|e| EngineError::Fatal(e.to_string()))?;
        }

        // Persist last_applied inside the same write txn — atomic with the data restore.
        if let Some(last) = metadata.last_included {
            meta_db
                .put(
                    &mut wtxn,
                    META_LAST_APPLIED_INDEX,
                    &last.index.to_le_bytes(),
                )
                .map_err(|e| EngineError::Fatal(e.to_string()))?;
            meta_db
                .put(&mut wtxn, META_LAST_APPLIED_TERM, &last.term.to_le_bytes())
                .map_err(|e| EngineError::Fatal(e.to_string()))?;
        }

        wtxn.commit().map_err(|e| EngineError::Fatal(e.to_string()))?;
        info!(?snapshot_path, "Snapshot applied");
        Ok(())
    }
}

impl Drop for LmdbStateMachine {
    fn drop(&mut self) {
        let log_id = self.last_applied();
        if let Err(e) = self.persist_last_applied(log_id) {
            eprintln!("LmdbStateMachine: failed to persist last_applied on drop: {e}");
        }
    }
}

impl LmdbStateMachine {
    /// LMDB rejects a zero-length key as a range bound (MDB_BAD_VALSIZE). Every
    /// scan method that takes a caller-supplied start/prefix must guard on it
    /// before touching `kv_db` — this is the single place that constructs the
    /// "matched nothing" result, so no call site duplicates the ScanResult shape.
    /// `scan_all` is exempt: it never constructs a start key at all (`Bound::Unbounded`).
    fn empty_key_guard(&self) -> ScanResult {
        let revision = self.last_applied_index.load(Ordering::SeqCst);
        ScanResult {
            entries: vec![],
            revision,
        }
    }

    fn scan_range_internal(
        &self,
        start: std::ops::Bound<&[u8]>,
        end: Option<&[u8]>,
        limit: Option<usize>,
    ) -> Result<ScanResult, EngineError> {
        let rtxn = self.env.read_txn().map_err(lmdb_err)?;
        let revision = self.last_applied_index.load(Ordering::SeqCst);
        let cap = limit.unwrap_or(usize::MAX);
        let mut entries = Vec::new();

        let range = (start, Bound::Unbounded::<&[u8]>);
        let iter = self.kv_db.range(&rtxn, &range).map_err(lmdb_err)?;

        for result in iter {
            if entries.len() >= cap {
                break;
            }
            let (k, v) = result.map_err(lmdb_err)?;
            if let Some(e) = end {
                if k >= e {
                    break;
                }
            }
            if let Some(payload) = decode_live_payload(v)? {
                entries.push((Bytes::copy_from_slice(k), Bytes::copy_from_slice(payload)));
            }
        }

        Ok(ScanResult { entries, revision })
    }

    /// Scan keys in `[start, end)`, returning at most `limit` entries.
    pub fn scan_range(
        &self,
        start: &[u8],
        end: Option<&[u8]>,
        limit: Option<usize>,
    ) -> Result<ScanResult, EngineError> {
        if start.is_empty() {
            return Ok(self.empty_key_guard());
        }

        self.scan_range_internal(Bound::Included(start), end, limit)
    }

    /// Scan all keys, returning at most `limit` entries.
    pub fn scan_all(
        &self,
        limit: Option<usize>,
    ) -> Result<ScanResult, EngineError> {
        self.scan_range_internal(Bound::Unbounded, None, limit)
    }

    /// Scan keys sharing `prefix`, optionally starting after `after` (exclusive) for pagination.
    pub fn scan_prefix_bounded(
        &self,
        prefix: &[u8],
        after: Option<&[u8]>,
        limit: Option<usize>,
    ) -> Result<ScanResult, EngineError> {
        if prefix.is_empty() {
            return Ok(self.empty_key_guard());
        }

        let end = next_prefix(prefix);
        let start = match after {
            Some(a) => Bound::Excluded(a),
            None => Bound::Included(prefix),
        };
        self.scan_range_internal(start, end.as_deref(), limit)
    }

    /// Scan keys in `[start, end)` in descending order, returning at most `limit` entries.
    ///
    /// Scans forward with a sliding window of size `limit`, so memory usage is
    /// O(limit), not O(range size). Full range is always traversed on disk.
    pub fn scan_range_rev(
        &self,
        start: &[u8],
        end: &[u8],
        limit: Option<usize>,
    ) -> Result<ScanResult, EngineError> {
        if start.is_empty() {
            return Ok(self.empty_key_guard());
        }

        let rtxn = self.env.read_txn().map_err(lmdb_err)?;
        let revision = self.last_applied_index.load(Ordering::SeqCst);
        let cap = limit.unwrap_or(usize::MAX);

        let range = (Bound::Included(start), Bound::Excluded(end));
        let iter = self.kv_db.range(&rtxn, &range).map_err(lmdb_err)?;

        let mut window: VecDeque<(Bytes, Bytes)> = VecDeque::new();
        for result in iter {
            let (k, v) = result.map_err(lmdb_err)?;
            if let Some(payload) = decode_live_payload(v)? {
                if window.len() >= cap {
                    window.pop_front();
                }
                window.push_back((Bytes::copy_from_slice(k), Bytes::copy_from_slice(payload)));
            }
        }

        let entries: Vec<_> = window.into_iter().rev().collect();
        Ok(ScanResult { entries, revision })
    }
}

#[async_trait]
impl StateMachine for LmdbStateMachine {
    async fn start(&self) -> Result<(), EngineError> {
        self.running.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn stop(&self) -> Result<(), EngineError> {
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    fn get(
        &self,
        key_buffer: &[u8],
    ) -> Result<Option<Bytes>, EngineError> {
        let rtxn = self.env.read_txn().map_err(lmdb_err)?;
        let Some(raw_bytes) = self.kv_db.get(&rtxn, key_buffer).map_err(lmdb_err)? else {
            return Ok(None);
        };
        if let Some(payload) = decode_live_payload(raw_bytes)? {
            Ok(Some(Bytes::copy_from_slice(payload)))
        } else {
            drop(rtxn);
            let mut wtxn = self.env.write_txn().map_err(lmdb_err)?;
            self.kv_db.delete(&mut wtxn, key_buffer).map_err(lmdb_err)?;
            wtxn.commit().map_err(lmdb_err)?;
            Ok(None)
        }
    }

    async fn apply_chunk(
        &self,
        chunk: &[ApplyEntry],
    ) -> Result<Vec<ApplyResult>, EngineError> {
        let mut wtxn = self.env.write_txn().map_err(lmdb_err)?;
        let mut results = Vec::with_capacity(chunk.len());
        let mut highest: Option<LogId> = None;

        for entry in chunk {
            let result = match &entry.command {
                Command::Insert {
                    key,
                    value,
                    ttl_secs,
                } => {
                    // TTL: d-engine enforces expiry at the Raft layer; LMDB stores the raw
                    // value unchanged. If d-lmdb needs LMDB-native TTL, implement a
                    // background reaper using the ttl_secs field from here.
                    let _ = ttl_secs;
                    self.kv_db.put(&mut wtxn, key.as_ref(), value.as_ref()).map_err(lmdb_err)?;
                    ApplyResult::success(entry.index)
                }
                Command::Delete { key } => {
                    self.kv_db.delete(&mut wtxn, key.as_ref()).map_err(lmdb_err)?;
                    ApplyResult::success(entry.index)
                }
                Command::CompareAndSwap {
                    key,
                    expected,
                    value,
                } => {
                    let current = self.kv_db.get(&wtxn, key.as_ref()).map_err(lmdb_err)?;
                    let matches = match (current, expected) {
                        (None, None) => true,
                        (Some(c), Some(e)) => c == e.as_ref(),
                        _ => false,
                    };
                    if matches {
                        self.kv_db
                            .put(&mut wtxn, key.as_ref(), value.as_ref())
                            .map_err(lmdb_err)?;
                        ApplyResult::success(entry.index)
                    } else {
                        ApplyResult::failure(entry.index)
                    }
                }
                Command::Batch { ops } => {
                    for op in ops {
                        match op {
                            d_engine::BatchOp::Insert { key, value } => {
                                self.kv_db
                                    .put(&mut wtxn, key.as_ref(), value.as_ref())
                                    .map_err(lmdb_err)?;
                            }
                            d_engine::BatchOp::Delete { key } => {
                                self.kv_db.delete(&mut wtxn, key.as_ref()).map_err(lmdb_err)?;
                            }
                        }
                    }
                    ApplyResult::success(entry.index)
                }
                Command::Noop => ApplyResult::success(entry.index),
            };

            results.push(result);

            // Track term for this index (in-memory, not persisted across restarts).
            self.entry_terms
                .lock()
                .expect("entry_terms lock poisoned")
                .insert(entry.index, entry.term);

            highest = Some(LogId {
                index: entry.index,
                term: entry.term,
            });
        }

        wtxn.commit().map_err(lmdb_err)?;

        if let Some(log_id) = highest {
            self.update_last_applied(log_id);
        }

        Ok(results)
    }

    fn len(&self) -> usize {
        let rtxn = match self.env.read_txn() {
            Ok(t) => t,
            Err(_) => return 0,
        };
        self.kv_db.len(&rtxn).unwrap_or(0) as usize
    }

    fn update_last_applied(
        &self,
        last_applied: LogId,
    ) {
        self.last_applied_index.store(last_applied.index, Ordering::SeqCst);
        self.last_applied_term.store(last_applied.term, Ordering::SeqCst);
    }

    fn last_applied(&self) -> LogId {
        LogId {
            index: self.last_applied_index.load(Ordering::SeqCst),
            term: self.last_applied_term.load(Ordering::SeqCst),
        }
    }

    fn persist_last_applied(
        &self,
        last_applied: LogId,
    ) -> Result<(), EngineError> {
        let mut wtxn = self.env.write_txn().map_err(lmdb_err)?;
        self.meta_db
            .put(
                &mut wtxn,
                META_LAST_APPLIED_INDEX,
                &last_applied.index.to_le_bytes(),
            )
            .map_err(lmdb_err)?;
        self.meta_db
            .put(
                &mut wtxn,
                META_LAST_APPLIED_TERM,
                &last_applied.term.to_le_bytes(),
            )
            .map_err(lmdb_err)?;
        wtxn.commit().map_err(lmdb_err)?;
        self.update_last_applied(last_applied);
        Ok(())
    }

    fn update_last_snapshot_metadata(
        &self,
        metadata: &SnapshotMetadata,
    ) -> Result<(), EngineError> {
        *self.snapshot_meta.lock().expect("snapshot_meta lock poisoned") = Some(metadata.clone());
        Ok(())
    }

    fn snapshot_metadata(&self) -> Option<SnapshotMetadata> {
        self.snapshot_meta.lock().expect("snapshot_meta lock poisoned").clone()
    }

    fn persist_last_snapshot_metadata(
        &self,
        metadata: &SnapshotMetadata,
    ) -> Result<(), EngineError> {
        // TODO: serialize metadata and store in meta_db under "snapshot_meta" key.
        self.update_last_snapshot_metadata(metadata)
    }

    async fn apply_snapshot_from_file(
        &self,
        metadata: &SnapshotMetadata,
        snapshot_path: std::path::PathBuf,
    ) -> Result<(), EngineError> {
        self.running.store(false, Ordering::SeqCst);

        let result = tokio::task::spawn_blocking({
            let env = self.env.clone();
            let kv_db = self.kv_db;
            let meta_db = self.meta_db;
            let map_size = self.map_size;
            let metadata = metadata.clone();
            move || Self::restore_blocking(env, kv_db, meta_db, map_size, metadata, snapshot_path)
        })
        .await
        .map_err(|e| EngineError::Fatal(e.to_string()))?;

        // Update in-memory atomics after blocking I/O completes.
        if result.is_ok() {
            if let Some(last) = metadata.last_included {
                self.last_applied_index.store(last.index, Ordering::SeqCst);
                self.last_applied_term.store(last.term, Ordering::SeqCst);
            }
            self.update_last_snapshot_metadata(metadata)?;
            self.entry_terms.lock().expect("entry_terms poisoned").clear();
        }

        // Always resume serving: on failure the leader will re-send the snapshot.
        self.running.store(true, Ordering::SeqCst);
        result
    }

    async fn generate_snapshot_data(
        &self,
        new_snapshot_dir: std::path::PathBuf,
        last_included: LogId,
    ) -> Result<Bytes, EngineError> {
        tokio::fs::create_dir_all(&new_snapshot_dir)
            .await
            .map_err(|e| EngineError::Fatal(e.to_string()))?;

        let env = self.env.clone();
        let snapshot_file = new_snapshot_dir.join("data.mdb");
        tokio::task::spawn_blocking(move || {
            env.copy_to_file(&snapshot_file, CompactionOption::Enabled)
                .map(|_| ())
                .map_err(|e| EngineError::Fatal(e.to_string()))
        })
        .await
        .map_err(|e| EngineError::Fatal(e.to_string()))??;

        let checksum = Bytes::from_static(&[0u8; 32]);
        let snap_meta = SnapshotMetadata {
            last_included: Some(last_included),
            checksum: checksum.clone(),
        };
        self.persist_last_snapshot_metadata(&snap_meta)?;

        info!(?new_snapshot_dir, "Snapshot generated");
        Ok(checksum)
    }

    fn save_hard_state(&self) -> Result<(), EngineError> {
        // Hard state (term + vote) is persisted by d-engine's storage engine (WAL),
        // not the state machine. This is a no-op for d-lmdb.
        Ok(())
    }

    fn flush(&self) -> Result<(), EngineError> {
        // LMDB in write-through mode syncs to OS on commit. In NOSYNC mode, call
        // env.force_sync() here. For now, rely on the default sync-on-commit behavior.
        Ok(())
    }

    async fn flush_async(&self) -> Result<(), EngineError> {
        self.flush()
    }

    async fn reset(&self) -> Result<(), EngineError> {
        let mut wtxn = self.env.write_txn().map_err(lmdb_err)?;
        self.kv_db.clear(&mut wtxn).map_err(lmdb_err)?;
        self.meta_db.clear(&mut wtxn).map_err(lmdb_err)?;
        self.internal_db.clear(&mut wtxn).map_err(lmdb_err)?;
        wtxn.commit().map_err(lmdb_err)?;

        self.last_applied_index.store(0, Ordering::SeqCst);
        self.last_applied_term.store(0, Ordering::SeqCst);
        *self.snapshot_meta.lock().expect("snapshot_meta poisoned") = None;
        self.entry_terms.lock().expect("entry_terms poisoned").clear();

        Ok(())
    }

    fn scan_prefix(
        &self,
        prefix: &[u8],
    ) -> Result<ScanResult, EngineError> {
        self.scan_prefix_bounded(prefix, None, None)
    }
}

// ---- helpers ----------------------------------------------------------------

fn lmdb_err(e: heed::Error) -> EngineError {
    StorageError::StateMachineError(e.to_string()).into()
}
fn codec_err(e: impl std::fmt::Display) -> EngineError {
    StorageError::StateMachineError(e.to_string()).into()
}
fn u64_from_bytes(b: &[u8]) -> u64 {
    let arr: [u8; 8] = b.try_into().unwrap_or([0u8; 8]);
    u64::from_le_bytes(arr)
}

/// Returns the lexicographic successor of `prefix` (the smallest byte string > prefix
/// that is not a prefix of prefix), or None if prefix ends with 0xFF bytes.
fn next_prefix(prefix: &[u8]) -> Option<Vec<u8>> {
    let mut next = prefix.to_vec();
    loop {
        match next.last_mut() {
            None => return None,
            Some(b) if *b < 0xFF => {
                *b += 1;
                return Some(next);
            }
            Some(_) => {
                next.pop();
            }
        }
    }
}

fn decode_live_payload(bytes: &[u8]) -> Result<Option<&[u8]>, EngineError> {
    let decoded = Value::decode(bytes).map_err(|_| codec_err("corrupt value: invalid tag byte"))?;
    match decoded {
        Value::Raw(payload) => Ok(Some(payload)),
        Value::Ttl {
            expires_at,
            payload,
        } => {
            let now = unix_now_secs();
            if expires_at <= now {
                Ok(None)
            } else {
                Ok(Some(payload))
            }
        }
    }
}

#[cfg(test)]
#[path = "state_machine_test.rs"]
mod state_machine_test;

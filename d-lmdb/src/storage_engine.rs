use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use d_engine::Error;
use d_engine::HardState;
use d_engine::LogStore;
use d_engine::MetaStore;
use d_engine::StorageEngine;
use d_engine::StorageError;
use d_engine::common::Entry;
use d_engine::common::LogId;
use heed::Database;
use heed::Env;
use heed::EnvOpenOptions;
use heed::types::Bytes as LmdbBytes;
use prost::Message;

const DB_LOG: &str = "log";
const DB_META: &str = "meta";

const KEY_HARD_STATE: &[u8] = b"hard_state";
const KEY_PURGE_BOUNDARY: &[u8] = b"purge_boundary";

const WAL_MAP_SIZE: usize = 128 * 1024 * 1024;

#[derive(Debug)]
pub(crate) struct LmdbWal {
    env: Env,
    log_db: Database<LmdbBytes, LmdbBytes>,
    meta_db: Database<LmdbBytes, LmdbBytes>,
    last_index: AtomicU64,
}

#[derive(Debug)]
pub(crate) struct LmdbStorageEngine {
    wal: Arc<LmdbWal>,
}

impl LmdbStorageEngine {
    pub(crate) fn new(raft_dir: PathBuf) -> Result<Self, Error> {
        std::fs::create_dir_all(&raft_dir).map_err(|e| StorageError::DbError(e.to_string()))?;

        let env = unsafe {
            EnvOpenOptions::new()
                .map_size(WAL_MAP_SIZE)
                .max_dbs(2)
                .open(&raft_dir)
                .map_err(db_err)?
        };

        let (log_db, meta_db, last_index) = {
            let mut wtxn = env.write_txn().map_err(db_err)?;
            let log_db: Database<LmdbBytes, LmdbBytes> =
                env.create_database(&mut wtxn, Some(DB_LOG)).map_err(db_err)?;
            let meta_db: Database<LmdbBytes, LmdbBytes> =
                env.create_database(&mut wtxn, Some(DB_META)).map_err(db_err)?;

            // Recover last_index by reading the last key in the log DB.
            let last_index = log_db
                .last(&wtxn)
                .map_err(db_err)?
                .and_then(|(k, _)| {
                    let arr: [u8; 8] = k.try_into().ok()?;
                    Some(u64::from_be_bytes(arr))
                })
                .unwrap_or(0);

            wtxn.commit().map_err(db_err)?;
            (log_db, meta_db, last_index)
        };

        Ok(Self {
            wal: Arc::new(LmdbWal {
                env,
                log_db,
                meta_db,
                last_index: AtomicU64::new(last_index),
            }),
        })
    }
}

impl StorageEngine for LmdbStorageEngine {
    type LogStore = LmdbWal;
    type MetaStore = LmdbWal;

    fn log_store(&self) -> Arc<LmdbWal> {
        Arc::clone(&self.wal)
    }

    fn meta_store(&self) -> Arc<LmdbWal> {
        Arc::clone(&self.wal)
    }
}

#[async_trait]
impl LogStore for LmdbWal {
    async fn persist_entries(
        &self,
        entries: Vec<Entry>,
    ) -> Result<(), Error> {
        if entries.is_empty() {
            return Ok(());
        }

        let mut max_index = 0u64;
        let mut wtxn = self.env.write_txn().map_err(db_err)?;

        for entry in &entries {
            let key = entry.index.to_be_bytes();
            let value = entry.encode_to_vec();
            self.log_db.put(&mut wtxn, &key, &value).map_err(db_err)?;
            max_index = max_index.max(entry.index);
        }

        wtxn.commit().map_err(db_err)?;
        self.last_index.fetch_max(max_index, Ordering::SeqCst);
        Ok(())
    }

    async fn entry(
        &self,
        index: u64,
    ) -> Result<Option<Entry>, Error> {
        let rtxn = self.env.read_txn().map_err(db_err)?;
        let key = index.to_be_bytes();
        match self.log_db.get(&rtxn, &key).map_err(db_err)? {
            Some(bytes) => Entry::decode(bytes)
                .map(Some)
                .map_err(|e| StorageError::SerializationError(e.to_string()).into()),
            None => Ok(None),
        }
    }

    fn get_entries(
        &self,
        range: RangeInclusive<u64>,
    ) -> Result<Vec<Entry>, Error> {
        let rtxn = self.env.read_txn().map_err(db_err)?;

        let start = range.start().to_be_bytes();

        let scan_range = (
            std::ops::Bound::Included(start.as_slice()),
            std::ops::Bound::Unbounded,
        );

        let mut entries = Vec::new();
        for result in self.log_db.range(&rtxn, &scan_range).map_err(db_err)? {
            let (k, v) = result.map_err(db_err)?;
            let arr: [u8; 8] = k
                .try_into()
                .map_err(|_| StorageError::DbError("invalid log key length".to_string()))?;
            let idx = u64::from_be_bytes(arr);
            if idx > *range.end() {
                break;
            }
            let entry =
                Entry::decode(v).map_err(|e| StorageError::SerializationError(e.to_string()))?;
            entries.push(entry);
        }
        Ok(entries)
    }

    async fn purge(
        &self,
        cutoff_index: LogId,
    ) -> Result<(), Error> {
        // Compute end key: first key AFTER cutoff (exclusive upper bound).
        let start = 0u64.to_be_bytes();
        let end = cutoff_index.index.saturating_add(1).to_be_bytes();

        let mut wtxn = self.env.write_txn().map_err(db_err)?;

        let del_range = (
            std::ops::Bound::Included(start.as_slice()),
            std::ops::Bound::Excluded(end.as_slice()),
        );
        self.log_db.delete_range(&mut wtxn, &del_range).map_err(db_err)?;

        // Persist purge boundary for crash recovery (BufferedRaftLog reads this on restart).
        let encoded = cutoff_index.encode_to_vec();
        self.meta_db.put(&mut wtxn, KEY_PURGE_BOUNDARY, &encoded).map_err(db_err)?;

        wtxn.commit().map_err(db_err)?;
        Ok(())
    }

    async fn truncate(
        &self,
        from_index: u64,
    ) -> Result<(), Error> {
        let start = from_index.to_be_bytes();

        let mut wtxn = self.env.write_txn().map_err(db_err)?;

        let del_range = (
            std::ops::Bound::Included(start.as_slice()),
            std::ops::Bound::Unbounded,
        );
        self.log_db.delete_range(&mut wtxn, &del_range).map_err(db_err)?;

        // Recompute last_index from the new tail.
        let new_last = self
            .log_db
            .last(&wtxn)
            .map_err(db_err)?
            .and_then(|(k, _)| {
                let arr: [u8; 8] = k.try_into().ok()?;
                Some(u64::from_be_bytes(arr))
            })
            .unwrap_or(0);

        wtxn.commit().map_err(db_err)?;
        self.last_index.store(new_last, Ordering::SeqCst);
        Ok(())
    }

    fn is_write_durable(&self) -> bool {
        // LMDB commits are crash-safe by default (MDB_NOSYNC not set).
        true
    }

    fn last_index(&self) -> u64 {
        self.last_index.load(Ordering::SeqCst)
    }

    fn load_purge_boundary(&self) -> Result<Option<LogId>, Error> {
        let rtxn = self.env.read_txn().map_err(db_err)?;
        match self.meta_db.get(&rtxn, KEY_PURGE_BOUNDARY).map_err(db_err)? {
            Some(bytes) => LogId::decode(bytes)
                .map(Some)
                .map_err(|e| StorageError::SerializationError(e.to_string()).into()),
            None => Ok(None),
        }
    }

    async fn reset(&self) -> Result<(), Error> {
        let mut wtxn = self.env.write_txn().map_err(db_err)?;
        self.log_db.clear(&mut wtxn).map_err(db_err)?;
        wtxn.commit().map_err(db_err)?;
        self.last_index.store(0, Ordering::SeqCst);
        Ok(())
    }
}

impl MetaStore for LmdbWal {
    fn save_hard_state(
        &self,
        state: &HardState,
    ) -> Result<(), Error> {
        let serialized = bincode::serialize(state).map_err(StorageError::BincodeError)?;
        let mut wtxn = self.env.write_txn().map_err(db_err)?;
        self.meta_db.put(&mut wtxn, KEY_HARD_STATE, &serialized).map_err(db_err)?;
        wtxn.commit().map_err(db_err)?;
        Ok(())
    }

    fn load_hard_state(&self) -> Result<Option<HardState>, Error> {
        let rtxn = self.env.read_txn().map_err(db_err)?;
        match self.meta_db.get(&rtxn, KEY_HARD_STATE).map_err(db_err)? {
            Some(bytes) => {
                let state = bincode::deserialize(bytes).map_err(StorageError::BincodeError)?;
                Ok(Some(state))
            }
            None => Ok(None),
        }
    }
}

fn db_err(e: heed::Error) -> Error {
    StorageError::DbError(e.to_string()).into()
}

#[cfg(test)]
#[path = "storage_engine_test.rs"]
mod storage_engine_test;

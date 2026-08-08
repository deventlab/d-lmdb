//! Exclusive OS-level lock on this node's data_dir, held for the life of the process.
//!
//! Acquired before the LMDB/raft storage is opened. d-engine's `start_custom`/
//! `start_node` only lock internally, after the caller's storage engine and state
//! machine are already constructed — d-lmdb owns closing that gap for itself.

use std::fs::File;
use std::fs::OpenOptions;
use std::path::Path;

use crate::Error;
use crate::Result;

pub(crate) struct DataDirLock {
    _file: File,
}

impl DataDirLock {
    /// Acquires an exclusive lock on `dir`. Fails immediately if another
    /// process already holds it.
    pub(crate) fn acquire(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let lock_path = dir.join(".d-lmdb.lock");
        let file = OpenOptions::new().create(true).truncate(false).write(true).open(&lock_path)?;
        file.try_lock().map_err(|_| {
            Error::Path(format!(
                "data_dir {} is already in use by another process",
                dir.display()
            ))
        })?;
        Ok(Self { _file: file })
    }
}

#[cfg(test)]
#[path = "lock_test.rs"]
mod lock_test;

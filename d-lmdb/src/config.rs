use std::path::Path;
use std::path::PathBuf;

use config::Config;
use config::File;
use serde::Deserialize;

use crate::Error;
use crate::Result;

/// Configuration for a `DLmdb` node: data location, LMDB map size, and key/value size limits.
#[derive(Deserialize)]
pub(crate) struct DLmdbConfig {
    /// Root directory for all persisted data (Raft WAL + LMDB state machine)
    pub(crate) data_dir: PathBuf,
    /// LMDB memory-map size in bytes. Must be larger than the total dataset.
    /// Default: 10 GiB — adjust for your expected data volume.
    #[serde(default = "default_map_size_gb")]
    pub(crate) map_size_gb: usize,

    /// Maximum accepted key size in bytes. Default: 512.
    pub(crate) max_key_bytes: usize,
    /// Maximum accepted value size in bytes. Default: 1MB.
    pub(crate) max_value_bytes: usize,
}

impl DLmdbConfig {
    /// Build a config with default map size and key/value limits, rooted at `data_path`.
    pub(crate) fn new(data_path: impl AsRef<Path>) -> Self {
        Self {
            data_dir: data_path.as_ref().to_path_buf(),
            map_size_gb: 10 * 1024 * 1024 * 1024, //10 GiB
            max_key_bytes: default_max_key_bytes(),
            max_value_bytes: default_max_value_bytes(),
        }
    }

    /// Load a config from a TOML/YAML/JSON file (see [`config`] crate for supported formats).
    pub(crate) fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let path_str = path
            .as_ref()
            .to_str()
            .ok_or_else(|| Error::Path("path is not valid UTF-8".into()))?;

        let root: DLmdbFileRoot = Config::builder()
            .add_source(File::with_name(path_str))
            .build()?
            .try_deserialize()?;
        Ok(Self {
            data_dir: root.lmdb.data_dir,
            map_size_gb: root.lmdb.map_size_gb * 1024 * 1024 * 1024,
            max_key_bytes: root.lmdb.max_key_bytes,
            max_value_bytes: root.lmdb.max_value_bytes,
        })
    }
}

#[derive(Deserialize)]
struct LmdbFileSection {
    data_dir: PathBuf,
    #[serde(default = "default_map_size_gb")]
    map_size_gb: usize,
    #[serde(default = "default_max_key_bytes")]
    max_key_bytes: usize,
    #[serde(default = "default_max_value_bytes")]
    max_value_bytes: usize,
}

#[derive(Deserialize)]
struct DLmdbFileRoot {
    lmdb: LmdbFileSection,
}

fn default_map_size_gb() -> usize {
    10
}

/// default 512
fn default_max_key_bytes() -> usize {
    512
}

/// default 1MB
fn default_max_value_bytes() -> usize {
    1024 * 1024
}

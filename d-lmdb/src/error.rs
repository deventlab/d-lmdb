use config::ConfigError;
use thiserror::Error;

/// All errors `d-lmdb` can return.
///
/// Marked `#[non_exhaustive]`: new variants may be added in a minor release
/// without that being a breaking change. Match with a wildcard arm (`_ => ...`).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// A d-engine (Raft) operation failed.
    #[error("engine: {0}")]
    Engine(#[from] d_engine::Error),

    /// A d-engine client call failed.
    #[error("client: {0}")]
    Client(#[from] d_engine::ClientApiError),

    /// The underlying LMDB (heed) call failed.
    #[error("lmdb: {0}")]
    Lmdb(#[from] heed::Error),

    /// A filesystem/IO operation failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// The supplied key exceeds the 512-byte limit.
    #[error("key too large: {0} bytes (max 512)")]
    KeyTooLarge(usize),

    /// The supplied value exceeds the 1MB limit.
    #[error("value too large: {0} bytes (max 1MB)")]
    ValueTooLarge(usize),

    /// Stored data could not be decoded (corrupt or unexpected format).
    #[error("storage: {0}")]
    Storage(String),

    /// A filesystem path was not valid UTF-8.
    #[error("path is not valid UTF-8: {0}")]
    Path(String),

    /// Loading `DLmdbConfig` from a config file failed.
    #[error("config_error: {0}")]
    ConfigError(#[from] ConfigError),
}

/// Convenience alias for `Result<T, Error>`.
pub type Result<T> = std::result::Result<T, Error>;

impl From<Error> for d_engine::Error {
    fn from(e: Error) -> Self {
        d_engine::Error::Fatal(e.to_string())
    }
}

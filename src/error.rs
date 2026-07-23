use config::ConfigError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("engine: {0}")]
    Engine(#[from] d_engine::Error),

    #[error("client: {0}")]
    Client(#[from] d_engine::ClientApiError),

    #[error("lmdb: {0}")]
    Lmdb(#[from] heed::Error),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("key too large: {0} bytes (max 512)")]
    KeyTooLarge(usize),

    #[error("value too large: {0} bytes (max 1MB)")]
    ValueTooLarge(usize),

    #[error("storage: {0}")]
    Storage(String),

    #[error("path is not valid UTF-8: {0}")]
    Path(String),

    #[error("config_error: {0}")]
    ConfigError(#[from] ConfigError),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<Error> for d_engine::Error {
    fn from(e: Error) -> Self {
        d_engine::Error::Fatal(e.to_string())
    }
}

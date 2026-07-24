use std::sync::Arc;

use async_trait::async_trait;
use d_engine::storage_engine_test::StorageEngineBuilder;
use d_engine::storage_engine_test::StorageEngineTestSuite;
use d_engine::Error as EngineError;
use tempfile::TempDir;

use crate::storage_engine::LmdbStorageEngine;

struct LmdbStorageEngineBuilder {
    temp_dir: TempDir,
}

impl LmdbStorageEngineBuilder {
    fn new() -> Self {
        Self {
            temp_dir: TempDir::new().expect("temp dir"),
        }
    }
}

#[async_trait]
impl StorageEngineBuilder for LmdbStorageEngineBuilder {
    type Engine = LmdbStorageEngine;

    async fn build(&self) -> Result<Arc<Self::Engine>, EngineError> {
        let path = self.temp_dir.path().join("raft");
        let engine = LmdbStorageEngine::new(path)?;
        Ok(Arc::new(engine))
    }

    async fn cleanup(&self) -> Result<(), EngineError> {
        Ok(())
    }
}

#[tokio::test]
async fn test_lmdb_storage_engine_suite() {
    let builder = LmdbStorageEngineBuilder::new();
    StorageEngineTestSuite::run_all_tests(builder)
        .await
        .expect("LmdbStorageEngine must pass all tests");
}

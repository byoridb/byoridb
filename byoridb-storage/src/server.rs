// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Storage server implementation

use super::env::{StorageEnv, StorageEnvConfig};
use super::error::Result;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

/// Storage server status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerStatus {
    Uninitialized,
    Running,
    Stopped,
}

/// Storage server
pub struct StorageServer {
    env: Option<StorageEnv>,
    status: Arc<RwLock<ServerStatus>>,
}

impl StorageServer {
    pub fn new() -> Self {
        StorageServer {
            env: None,
            status: Arc::new(RwLock::new(ServerStatus::Uninitialized)),
        }
    }

    /// Start the storage server
    pub async fn start(&mut self, config: StorageEnvConfig) -> Result<()> {
        info!(
            "Starting storage server with data paths: {:?}",
            config.data_paths
        );

        let schema_provider = Arc::new(RwLock::new(byoridb_codec::MemorySchemaProvider::new()));

        let env = StorageEnv::new(config, schema_provider).await?;
        self.env = Some(env);

        *self.status.write().await = ServerStatus::Running;
        info!("Storage server started successfully");

        Ok(())
    }

    /// Stop the storage server
    pub async fn stop(&mut self) -> Result<()> {
        info!("Stopping storage server");

        // Cleanly checkpoint redb BEFORE dropping the env, so the next open()
        // finds a clean shutdown and skips the multi-minute full repair. Relying
        // on Drop is not enough: the KVStore is shared via Arc, so `env = None`
        // may not actually drop the Database, and even then Drop does not
        // guarantee the 2-phase allocator-state commit redb needs to mark clean.
        if let Some(env) = &self.env {
            env.kvstore.checkpoint().await?;
            info!("redb checkpointed (clean shutdown — next open skips repair)");
        }
        self.env = None;
        *self.status.write().await = ServerStatus::Stopped;

        info!("Storage server stopped");
        Ok(())
    }

    /// Get current server status
    pub async fn status(&self) -> ServerStatus {
        *self.status.read().await
    }

    /// Check if server is running
    pub async fn is_running(&self) -> bool {
        *self.status.read().await == ServerStatus::Running
    }

    /// Get the storage environment
    pub fn env(&self) -> Option<&StorageEnv> {
        self.env.as_ref()
    }
}

impl Default for StorageServer {
    fn default() -> Self {
        Self::new()
    }
}

// Use the processor defined in processor.rs
pub use crate::processor::StorageProcessor;

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use byoridb_kvstore::{KVStore, KVStoreError, Result as KvResult, VersionRecord};
    use std::sync::atomic::{AtomicBool, Ordering};

    struct CheckpointFault(AtomicBool);

    #[async_trait]
    impl KVStore for CheckpointFault {
        async fn checkpoint(&self) -> KvResult<()> {
            if self.0.load(Ordering::SeqCst) {
                Err(KVStoreError::StoreClosed)
            } else {
                Ok(())
            }
        }
        async fn get(&self, _: &[u8]) -> KvResult<Option<Vec<u8>>> {
            unreachable!()
        }
        async fn put(&self, _: &[u8], _: &[u8]) -> KvResult<()> {
            unreachable!()
        }
        async fn put_if_absent(&self, _: &[u8], _: &[u8]) -> KvResult<Option<Vec<u8>>> {
            unreachable!()
        }
        async fn delete(&self, _: &[u8]) -> KvResult<()> {
            unreachable!()
        }
        async fn batch_put(&self, _: Vec<(Vec<u8>, Vec<u8>)>) -> KvResult<()> {
            unreachable!()
        }
        async fn batch_delete(&self, _: Vec<Vec<u8>>) -> KvResult<()> {
            unreachable!()
        }
        async fn put_version(&self, _: &[u8], _: i64, _: i64, _: i64, _: &[u8]) -> KvResult<()> {
            unreachable!()
        }
        async fn scan_history(&self, _: &[u8]) -> KvResult<Vec<VersionRecord>> {
            unreachable!()
        }
        async fn scan_history_entity_keys(&self, _: &[u8]) -> KvResult<Vec<Vec<u8>>> {
            unreachable!()
        }
        async fn batch_put_version(
            &self,
            _: Vec<(Vec<u8>, i64, i64, i64, Vec<u8>)>,
        ) -> KvResult<()> {
            unreachable!()
        }
        async fn scan_prefix(&self, _: &[u8]) -> KvResult<Vec<(Vec<u8>, Vec<u8>)>> {
            unreachable!()
        }
        async fn batch_get(&self, _: &[Vec<u8>]) -> KvResult<Vec<Option<Vec<u8>>>> {
            unreachable!()
        }
    }

    #[tokio::test]
    async fn failed_checkpoint_is_observable_and_preserves_the_store_for_retry() {
        let store = Arc::new(CheckpointFault(AtomicBool::new(true)));
        let mut server = StorageServer {
            env: Some(StorageEnv {
                config: StorageEnvConfig::default(),
                kvstore: store.clone(),
                schema_provider: Arc::new(RwLock::new(byoridb_codec::MemorySchemaProvider::new())),
            }),
            status: Arc::new(RwLock::new(ServerStatus::Running)),
        };
        assert!(server.stop().await.is_err());
        assert_eq!(server.status().await, ServerStatus::Running);
        assert!(server.env().is_some());
        store.0.store(false, Ordering::SeqCst);
        server.stop().await.unwrap();
        assert_eq!(server.status().await, ServerStatus::Stopped);
        assert!(server.env().is_none());
    }
}

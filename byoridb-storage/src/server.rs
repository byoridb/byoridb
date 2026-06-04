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

        *self.status.write().await = ServerStatus::Stopped;
        self.env = None;

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

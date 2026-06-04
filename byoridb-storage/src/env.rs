// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Storage environment and context

use crate::error::{Result, StorageError};
use byoridb_codec::SchemaProvider;
use byoridb_kvstore::{KVStore, KVStoreOptions, WalKVStore};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

/// Storage environment configuration
#[derive(Debug, Clone)]
pub struct StorageEnvConfig {
    pub data_paths: Vec<PathBuf>,
    pub wal_path: Option<PathBuf>,
    pub listener_path: Option<PathBuf>,
    pub kvstore_opts: KVStoreOptions,
}

impl Default for StorageEnvConfig {
    fn default() -> Self {
        StorageEnvConfig {
            data_paths: vec![PathBuf::from("./data")],
            wal_path: None,
            listener_path: None,
            kvstore_opts: KVStoreOptions::default(),
        }
    }
}

/// Storage environment
pub struct StorageEnv {
    pub config: StorageEnvConfig,
    pub kvstore: Arc<dyn KVStore>,
    pub schema_provider: Arc<RwLock<dyn SchemaProvider>>,
}

impl StorageEnv {
    pub async fn new(
        config: StorageEnvConfig,
        schema_provider: Arc<RwLock<dyn SchemaProvider>>,
    ) -> Result<Self> {
        // Open KV store on first data path
        let data_path = config
            .data_paths
            .first()
            .ok_or_else(|| StorageError::Internal("No data path specified".to_string()))?;

        // Use WAL-enabled KVStore for durability
        info!("Opening WAL-enabled KVStore at {:?}", data_path);
        let kvstore = Arc::new(WalKVStore::open(data_path, config.kvstore_opts.clone())?);

        Ok(StorageEnv {
            config,
            kvstore,
            schema_provider,
        })
    }

    pub fn data_paths(&self) -> &[PathBuf] {
        &self.config.data_paths
    }

    pub fn wal_path(&self) -> Option<&PathBuf> {
        self.config.wal_path.as_ref()
    }
}

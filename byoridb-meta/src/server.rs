// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Meta server implementation

use crate::error::{MetaError, Result};
use crate::proto::meta_service_server::MetaServiceServer;
use crate::rpc::MetaRpcService;
use crate::service::MetaService;
use axum::extract::State;
use byoridb_kvstore::{KVStoreOptions, RedbKVStore};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tonic::transport::Server;
use tracing::{error, info};

/// Meta server configuration
#[derive(Debug, Clone)]
pub struct MetaServerConfig {
    /// Data directory for persistent storage
    pub data_path: PathBuf,
    /// gRPC server address
    pub grpc_addr: SocketAddr,
    /// HTTP server address (for health checks and REST API)
    pub http_addr: Option<SocketAddr>,
    /// Host address for advertising to clients
    pub advertise_host: String,
    /// Port for advertising to clients
    pub advertise_port: u32,
}

impl Default for MetaServerConfig {
    fn default() -> Self {
        Self {
            data_path: PathBuf::from("data/meta"),
            grpc_addr: "0.0.0.0:9559".parse().unwrap(),
            // Bind HTTP to localhost by default — exposes internal schema info.
            // Set to 0.0.0.0 explicitly only when external access is required.
            http_addr: Some("127.0.0.1:19559".parse().unwrap()),
            advertise_host: "127.0.0.1".to_string(),
            advertise_port: 9559,
        }
    }
}

/// Meta server
pub struct MetaServer {
    service: Arc<MetaService>,
    config: MetaServerConfig,
    running: Arc<RwLock<bool>>,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl MetaServer {
    /// Create a new meta server with default configuration
    pub async fn new(data_path: PathBuf) -> Result<Self> {
        let config = MetaServerConfig {
            data_path: data_path.clone(),
            ..Default::default()
        };
        Self::with_config(config).await
    }

    /// Create a new meta server with custom configuration
    pub async fn with_config(config: MetaServerConfig) -> Result<Self> {
        info!(
            "Initializing meta server with data path: {:?}",
            config.data_path
        );

        // Ensure data directory exists
        if !config.data_path.exists() {
            std::fs::create_dir_all(&config.data_path)
                .map_err(|e| MetaError::Storage(format!("Failed to create data dir: {}", e)))?;
        }

        let opts = KVStoreOptions::default();
        let kvstore = Arc::new(RedbKVStore::open(&config.data_path, opts)?);
        let service = Arc::new(MetaService::new(kvstore));

        Ok(MetaServer {
            service,
            config,
            running: Arc::new(RwLock::new(false)),
            shutdown_tx: None,
        })
    }

    /// Start the meta server (gRPC and optional HTTP)
    pub async fn start(&mut self) -> Result<()> {
        info!("Starting meta server on {}", self.config.grpc_addr);

        let mut running = self.running.write().await;
        if *running {
            return Err(MetaError::InvalidOperation(
                "Server already running".to_string(),
            ));
        }
        *running = true;
        drop(running);

        // Create gRPC service
        let rpc_service = MetaRpcService::new(
            Arc::clone(&self.service),
            self.config.advertise_host.clone(),
            self.config.advertise_port,
        );

        let grpc_addr = self.config.grpc_addr;
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        self.shutdown_tx = Some(shutdown_tx);

        // Spawn gRPC server
        let running_clone = Arc::clone(&self.running);
        tokio::spawn(async move {
            info!("Meta gRPC server listening on {}", grpc_addr);

            let result = Server::builder()
                .add_service(MetaServiceServer::new(rpc_service))
                .serve_with_shutdown(grpc_addr, async {
                    let _ = shutdown_rx.await;
                    info!("Meta gRPC server received shutdown signal");
                })
                .await;

            match result {
                Ok(()) => info!("Meta gRPC server stopped"),
                Err(e) => error!("Meta gRPC server error: {}", e),
            }

            let mut running = running_clone.write().await;
            *running = false;
        });

        // Start HTTP server if configured
        if let Some(http_addr) = self.config.http_addr {
            let service = Arc::clone(&self.service);
            tokio::spawn(async move {
                if let Err(e) = Self::run_http_server(http_addr, service).await {
                    error!("Meta HTTP server error: {}", e);
                }
            });
        }

        info!("Meta server started successfully");
        Ok(())
    }

    /// Run HTTP server for health checks and REST API
    async fn run_http_server(addr: SocketAddr, service: Arc<MetaService>) -> Result<()> {
        use axum::{routing::get, Router};

        let app = Router::new()
            .route("/health", get(|| async { "OK" }))
            .route("/api/v1/spaces", get(list_spaces_handler))
            .with_state(service);

        info!("Meta HTTP server listening on {}", addr);

        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| MetaError::Network(format!("Failed to bind HTTP: {}", e)))?;

        axum::serve(listener, app)
            .await
            .map_err(|e| MetaError::Network(format!("HTTP server error: {}", e)))?;

        Ok(())
    }

    /// Stop the meta server
    pub async fn stop(&mut self) -> Result<()> {
        info!("Stopping meta server");

        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }

        let mut running = self.running.write().await;
        *running = false;

        info!("Meta server stopped");
        Ok(())
    }

    /// Get the meta service
    pub fn service(&self) -> Arc<MetaService> {
        Arc::clone(&self.service)
    }

    /// Check if server is running
    pub async fn is_running(&self) -> bool {
        *self.running.read().await
    }

    /// Get server configuration
    pub fn config(&self) -> &MetaServerConfig {
        &self.config
    }
}

/// HTTP handler for listing spaces
async fn list_spaces_handler(
    State(service): axum::extract::State<Arc<MetaService>>,
) -> axum::Json<serde_json::Value> {
    match service.list_spaces().await {
        Ok(spaces) => {
            let space_list: Vec<serde_json::Value> = spaces
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "id": s.id,
                        "name": s.name,
                        "partition_num": s.partition_num,
                        "replica_factor": s.replica_factor,
                    })
                })
                .collect();
            axum::Json(serde_json::json!({
                "code": 0,
                "spaces": space_list
            }))
        }
        Err(e) => axum::Json(serde_json::json!({
            "code": -1,
            "error": e.to_string()
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_meta_server_creation() {
        let tmp_dir = tempdir().unwrap();
        let server = MetaServer::new(tmp_dir.path().to_path_buf()).await;
        assert!(server.is_ok());
    }

    #[tokio::test]
    async fn test_meta_server_config() {
        let config = MetaServerConfig::default();
        assert_eq!(config.grpc_addr.port(), 9559);
        assert_eq!(config.http_addr.unwrap().port(), 19559);
    }

    #[tokio::test]
    async fn test_meta_server_start_stop() {
        let tmp_dir = tempdir().unwrap();
        let config = MetaServerConfig {
            data_path: tmp_dir.path().to_path_buf(),
            grpc_addr: "127.0.0.1:0".parse().unwrap(), // Random port
            http_addr: None,                           // Disable HTTP for test
            ..Default::default()
        };

        let mut server = MetaServer::with_config(config).await.unwrap();

        // Server should not be running initially
        assert!(!server.is_running().await);

        // Start should succeed
        assert!(server.start().await.is_ok());

        // Give server time to start
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Stop should succeed
        assert!(server.stop().await.is_ok());
    }
}

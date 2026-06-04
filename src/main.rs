mod config;

use crate::config::AppConfig;
use tokio::sync::broadcast;
use tracing::{error, info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Initialize Logging
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    info!("Starting ByoriDB Server...");

    // 1.5. Initialize Metrics
    let _metrics_handle = byoridb_graph::init_metrics();
    info!("Metrics initialized (Prometheus endpoint: /metrics)");

    // 2. Load Configuration
    let config = AppConfig::load().expect("Failed to load configuration");
    info!("Configuration loaded: {:?}", config);

    // 3. Create shutdown channel
    let (shutdown_tx, _) = broadcast::channel::<()>(1);

    // 3.5. Cluster mode detection
    let is_cluster = !config.cluster.peers.is_empty();
    if is_cluster {
        info!(
            node_id = config.cluster.node_id,
            peers = ?config.cluster.peers,
            meta_addr = %config.cluster.meta_addr,
            advertise_addr = %config.cluster.advertise_addr,
            "Cluster mode: starting Meta server"
        );
        let meta_config = byoridb_meta::MetaServerConfig {
            data_path: std::path::PathBuf::from("data/meta"),
            grpc_addr: config
                .cluster
                .meta_addr
                .parse()
                .expect("Invalid cluster.meta_addr"),
            http_addr: None,
            advertise_host: config
                .cluster
                .advertise_addr
                .split_once(':')
                .map(|(h, _)| h)
                .unwrap_or("127.0.0.1")
                .to_string(),
            advertise_port: config
                .cluster
                .advertise_addr
                .split_once(':')
                .map(|(_, p)| p)
                .unwrap_or("9559")
                .parse()
                .unwrap_or(9559),
        };
        let mut meta_server = byoridb_meta::MetaServer::with_config(meta_config)
            .await
            .expect("Failed to initialize Meta server");
        let mut meta_shutdown_rx = shutdown_tx.subscribe();
        tokio::spawn(async move {
            tokio::select! {
                result = meta_server.start() => {
                    if let Err(e) = result { error!("Meta server failed: {}", e); }
                }
                _ = meta_shutdown_rx.recv() => {
                    info!("Meta server received shutdown signal");
                }
            }
        });
        info!("Meta server started (node_id={})", config.cluster.node_id);
    } else {
        info!("Standalone mode (single-node). Set BYORIDB__CLUSTER__PEERS to enable cluster.");
    }

    // 4. Start Storage Server
    let mut storage_server = byoridb_storage::StorageServer::new();
    let storage_config = byoridb_storage::env::StorageEnvConfig {
        data_paths: config
            .storage
            .data_paths
            .iter()
            .map(std::path::PathBuf::from)
            .collect(),
        wal_path: None,
        listener_path: None,
        kvstore_opts: byoridb_kvstore::KVStoreOptions::default(),
    };

    storage_server.start(storage_config).await?;
    info!("Storage server started");

    // Get KVStore from storage server
    let kvstore = storage_server
        .env()
        .expect("Storage server not started")
        .kvstore
        .clone();

    // 5. Start Graph Service (gRPC)
    let graph_addr = config.server.graph_addr;
    let graph_server = byoridb_graph::server::GraphServer::new(graph_addr, kvstore.clone());
    let mut graph_shutdown_rx = shutdown_tx.subscribe();

    let graph_handle = tokio::spawn(async move {
        info!("Graph Service starting on {}", graph_addr);

        tokio::select! {
            result = graph_server.start() => {
                if let Err(e) = result {
                    error!("Graph Service failed: {}", e);
                }
            }
            _ = graph_shutdown_rx.recv() => {
                info!("Graph Service received shutdown signal");
            }
        }
    });

    // 6. Start HTTP Service
    let http_addr = config.server.http_addr;
    let http_server = byoridb_graph::server::HttpServer::new(http_addr, kvstore.clone());
    let mut http_shutdown_rx = shutdown_tx.subscribe();

    let http_handle = tokio::spawn(async move {
        info!("HTTP Service starting on {}", http_addr);

        tokio::select! {
            result = http_server.start() => {
                if let Err(e) = result {
                    error!("HTTP Service failed: {}", e);
                }
            }
            _ = http_shutdown_rx.recv() => {
                info!("HTTP Service received shutdown signal");
            }
        }
    });

    // 7. Wait for shutdown signal
    info!("ByoriDB Server is ready. Press Ctrl+C to shutdown.");

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("Received Ctrl+C, initiating graceful shutdown...");
        }
        _ = terminate_signal() => {
            info!("Received SIGTERM, initiating graceful shutdown...");
        }
    }

    // 8. Graceful shutdown sequence
    info!("Shutting down services...");

    // Send shutdown signal to all services
    let _ = shutdown_tx.send(());

    // Wait for services to finish (with timeout)
    let shutdown_timeout = tokio::time::Duration::from_secs(30);

    info!("Waiting for Graph Service to shutdown...");
    if tokio::time::timeout(shutdown_timeout, graph_handle)
        .await
        .is_err()
    {
        warn!("Graph Service shutdown timed out");
    }

    info!("Waiting for HTTP Service to shutdown...");
    if tokio::time::timeout(shutdown_timeout, http_handle)
        .await
        .is_err()
    {
        warn!("HTTP Service shutdown timed out");
    }

    // Stop storage server (this syncs WAL)
    info!("Stopping Storage Server...");
    if let Err(e) = storage_server.stop().await {
        error!("Error stopping storage server: {}", e);
    }

    info!("ByoriDB Server shutdown complete");
    Ok(())
}

/// Wait for SIGTERM signal (Unix only)
#[cfg(unix)]
async fn terminate_signal() {
    use tokio::signal::unix::{signal, SignalKind};

    let mut sigterm = signal(SignalKind::terminate()).expect("Failed to register SIGTERM handler");

    sigterm.recv().await;
}

/// On non-Unix platforms, this never completes
#[cfg(not(unix))]
async fn terminate_signal() {
    std::future::pending::<()>().await
}

mod config;

use crate::config::AppConfig;
use anyhow::Context;
use clap::Parser;
use tokio::sync::broadcast;
use tracing::{error, info, warn};

/// Crate version, the commit it was built from, and the build profile. Both
/// extra values come from `build.rs`; the SHA is what identifies a deployed
/// artifact, since there is no maintained semver release line yet.
const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (commit ",
    env!("BYORIDB_GIT_SHA"),
    ", ",
    env!("BYORIDB_BUILD_PROFILE"),
    ")"
);

/// Shown after the generated usage. The server takes no options of its own —
/// every knob is a config file key or an environment variable — so `--help` is
/// only useful if it names them.
const CONFIG_HELP: &str = "\
Configuration is read from `byoridb.toml` (optional), then from environment
variables, which take precedence. Section keys use a double underscore:

  BYORIDB__SERVER__GRAPH_ADDR       gRPC listen address          [0.0.0.0:9669]
  BYORIDB__SERVER__HTTP_ADDR        HTTP listen address          [0.0.0.0:19669]
  BYORIDB__SERVER__STORAGE_ADDR     storage RPC listen address   [0.0.0.0:44500]
  BYORIDB__STORAGE__DATA_PATHS      comma-separated data dirs    [data/storage]
  BYORIDB__CLUSTER__PEERS           comma-separated meta peers   [empty]
  BYORIDB__CLUSTER__NODE_ID         raft node id                 [1]
  BYORIDB__CLUSTER__META_ADDR       meta gRPC listen address     [0.0.0.0:9559]
  BYORIDB__CLUSTER__ADVERTISE_ADDR  advertised meta address      [127.0.0.1:9559]

Other environment variables:

  BYORIDB_ROOT_PASSWORD  required; the server refuses to start without a
                         non-blank value
  BYORIDB_CACHE_SIZE_MB  redb page cache size                            [256]
  BYORIDB_DURABILITY     none|relaxed|eventual drops the per-commit fsync for
                         bulk loading; anything else keeps immediate durability
  BYORIDB_MAX_MEMORY_MB  per-query result-memory cap                    [1024]
  BYORIDB_MAX_SCAN_LIMIT per-scan row cap                            [100_000]
  RUST_LOG               tracing filter

Leaving BYORIDB__CLUSTER__PEERS empty selects single-node mode, which is the
only supported deployment. See docs/PLAN.md before configuring peers.";

#[derive(Parser)]
#[command(
    name = "byoridb-server",
    version = VERSION,
    about = "ByoriDB standalone server (gRPC + HTTP)",
    after_help = CONFIG_HELP
)]
struct Cli {}

fn validate_root_password(value: Option<String>) -> anyhow::Result<String> {
    match value {
        Some(password) if !password.trim().is_empty() => Ok(password),
        _ => anyhow::bail!(
            "{} must be set to a non-blank value",
            byoridb_graph::auth::ROOT_PASSWORD_ENV
        ),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 0. Parse arguments before anything observable happens. `--version` and
    // `--help` must answer without initializing logging, resolving credentials,
    // opening storage, or binding a listener, and an unrecognized flag must fail
    // instead of silently starting a server against default configuration.
    Cli::parse();

    // 1. Initialize Logging
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    info!("Starting ByoriDB Server...");

    // 1.5. Initialize Metrics
    let _metrics_handle = byoridb_graph::init_metrics();
    info!("Metrics initialized (Prometheus endpoint: /metrics)");

    // 2. Load Configuration
    let config = AppConfig::load().context("Failed to load configuration")?;
    info!("Configuration loaded: {:?}", config);

    // A network server must never start with an unknown or logged bootstrap
    // credential. Embedded callers may choose their own AuthManager, but the
    // standalone process requires an explicit secret and fails before opening
    // listeners or storage when it is absent.
    let root_password =
        validate_root_password(std::env::var(byoridb_graph::auth::ROOT_PASSWORD_ENV).ok())?;

    // 3. Create shutdown channel + shared readiness/drain state.
    // The gRPC and HTTP services both report in-flight queries into this
    // state; the signal handler uses it to fail readiness and drain before
    // tearing the servers down.
    let (shutdown_tx, _) = broadcast::channel::<()>(1);
    let shutdown_state = std::sync::Arc::new(byoridb_graph::ShutdownState::new());

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
            grpc_addr: config.cluster.meta_addr.parse().with_context(|| {
                format!("Invalid cluster.meta_addr: {}", config.cluster.meta_addr)
            })?,
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
            .context("Failed to initialize Meta server")?;
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
    // redb page cache size, overridable via BYORIDB_CACHE_SIZE_MB (default 256MB).
    // Sizing the cache near the working set keeps reads in memory and avoids the
    // disk-IOPS wall that throttles bulk loads once data exceeds the cache.
    let cache_size_mb: usize = std::env::var("BYORIDB_CACHE_SIZE_MB")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&mb| mb > 0)
        .unwrap_or(256);
    // BYORIDB_DURABILITY=none|relaxed → relaxed durability (no per-commit fsync,
    // periodic checkpoint) for fast bulk loading. Default: Immediate (fsync per
    // commit). Crash under relaxed loses recent commits — only safe for
    // re-loadable bulk imports, NOT steady-state serving.
    let relaxed_durability = std::env::var("BYORIDB_DURABILITY")
        .map(|v| matches!(v.to_lowercase().as_str(), "none" | "relaxed" | "eventual"))
        .unwrap_or(false);
    info!(
        "redb page cache: {} MB, durability: {}",
        cache_size_mb,
        if relaxed_durability {
            "relaxed (bulk load)"
        } else {
            "immediate"
        }
    );
    let storage_config = byoridb_storage::env::StorageEnvConfig {
        data_paths: config
            .storage
            .data_paths
            .iter()
            .map(std::path::PathBuf::from)
            .collect(),
        wal_path: None,
        listener_path: None,
        kvstore_opts: byoridb_kvstore::KVStoreOptions {
            cache_size: cache_size_mb * 1024 * 1024,
            use_fsync: !relaxed_durability,
            ..Default::default()
        },
    };

    storage_server.start(storage_config).await?;
    info!("Storage server started");

    // Get KVStore from storage server
    let kvstore = storage_server
        .env()
        .context("Storage server did not expose its environment after startup")?
        .kvstore
        .clone();

    // 5. Construct one graph service for both protocols. This is the security
    // boundary: HTTP and gRPC must share users and bearer sessions rather than
    // generating separate root credentials and disconnected auth caches.
    let graph_service = std::sync::Arc::new(
        byoridb_graph::GraphService::with_auth(
            kvstore.clone(),
            byoridb_graph::AuthManager::try_with_config(
                &root_password,
                std::time::Duration::from_secs(byoridb_graph::auth::DEFAULT_SESSION_TTL_SECS),
            )?,
        )
        .with_shutdown_state(shutdown_state.clone()),
    );
    graph_service.hydrate_persisted_users().await?;
    drop(root_password);

    // 5.1 Start Graph Service (gRPC)
    let graph_addr = config.server.graph_addr;
    let graph_server =
        byoridb_graph::server::GraphServer::with_service(graph_addr, graph_service.clone());
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

    // 6. Start HTTP Service over the same authentication/session state.
    let http_addr = config.server.http_addr;
    let http_server =
        byoridb_graph::server::HttpServer::with_service(http_addr, graph_service.clone());
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
    //
    // Order matters: fail readiness first (k8s stops routing, new queries are
    // rejected with a clear error), then drain in-flight queries, and only
    // then stop the servers — so running queries/loads finish instead of
    // being cut mid-write.
    shutdown_state.stop_accepting();
    info!("Readiness set to NOT READY; new queries are rejected. Draining in-flight queries...");

    let drain_timeout = tokio::time::Duration::from_secs(25);
    let drained = shutdown_state
        .drain(drain_timeout, tokio::time::Duration::from_millis(200))
        .await;
    if drained {
        info!("All in-flight queries drained");
    } else {
        warn!(
            "Drain timed out after {:?}; {} query(ies) still running will be cut",
            drain_timeout,
            shutdown_state.active_queries()
        );
    }

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

    // Stop storage server. This checkpoints redb (2-phase Immediate commit) so
    // the next startup finds a clean shutdown and skips the full-repair scan,
    // which on a large dataset takes many minutes (2026-06-26 incident).
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

    match signal(SignalKind::terminate()) {
        Ok(mut sigterm) => {
            sigterm.recv().await;
        }
        Err(error) => {
            error!(err = %error, "Failed to register SIGTERM handler");
            // A registration failure is not a termination request. Keep this
            // branch pending so Ctrl+C remains available to stop the server.
            std::future::pending::<()>().await;
        }
    }
}

/// On non-Unix platforms, this never completes
#[cfg(not(unix))]
async fn terminate_signal() {
    std::future::pending::<()>().await
}

#[cfg(test)]
mod tests {
    use super::validate_root_password;

    #[test]
    fn root_password_is_required_and_must_not_be_blank() {
        assert!(validate_root_password(None).is_err());
        assert!(validate_root_password(Some(String::new())).is_err());
        assert!(validate_root_password(Some(" \t\r\n".to_string())).is_err());
        assert_eq!(
            validate_root_password(Some(" configured-secret ".to_string())).unwrap(),
            " configured-secret "
        );
    }
}

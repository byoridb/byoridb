mod config;

use crate::config::AppConfig;
use anyhow::Context;
use clap::Parser;
use tokio::sync::broadcast;
use tracing::{error, info, warn};

#[path = "version.rs"]
mod version;
use version::VERSION;

/// Shown after the generated usage. The server takes no options of its own —
/// every knob is a config file key or an environment variable — so `--help` is
/// only useful if it names them.
const CONFIG_HELP: &str = "\
Configuration is read from `byoridb.toml` (optional), then from environment
variables, which take precedence. Section keys use a double underscore:

  BYORIDB__SERVER__GRAPH_ADDR       gRPC listen address          [0.0.0.0:9669]
  BYORIDB__SERVER__HTTP_ADDR        HTTP listen address          [0.0.0.0:19669]
  BYORIDB__STORAGE__DATA_PATHS      comma-separated data dirs    [data/storage]

Other environment variables:

  BYORIDB_ROOT_PASSWORD  required; the server refuses to start without a
                         non-blank value
  BYORIDB_CACHE_SIZE_MB  redb page cache size                            [256]
  BYORIDB_DURABILITY     none|relaxed|eventual drops the per-commit fsync for
                         bulk loading; anything else keeps immediate durability
  BYORIDB_MAX_MEMORY_MB  per-query result-memory cap                    [1024]
  BYORIDB_MAX_SCAN_LIMIT per-scan row cap                            [100_000]
  RUST_LOG               tracing filter

BYORIDB__CLUSTER__PEERS must remain empty and all other cluster settings must
keep their defaults. The launcher supports single-node deployment and rejects
cluster configuration. Storage RPC is not exposed. See docs/PLAN.md.";

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

    // The advertised deployment is standalone. Starting only Meta while graph
    // and storage remain local would look like a functioning replicated server.
    anyhow::ensure!(
        config.cluster.is_standalone(),
        "Cluster deployment is not supported by this launcher; keep cluster settings at their defaults"
    );

    // Bind both protocols before storage initialization or any readiness claim.
    // A partial startup is an error, never a healthy process with a dead API.
    let graph_listener = tokio::net::TcpListener::bind(config.server.graph_addr)
        .await
        .context("Failed to bind gRPC listener")?;
    let http_listener = tokio::net::TcpListener::bind(config.server.http_addr)
        .await
        .context("Failed to bind HTTP listener")?;
    let graph_listen_addr = graph_listener.local_addr()?;
    let http_listen_addr = http_listener.local_addr()?;
    info!("Standalone mode (single-node)");

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
            // `with_auth` derives the graph session store's TTL from this one,
            // so a configured lifetime governs both stores instead of being
            // capped by the other's default.
            byoridb_graph::AuthManager::try_with_throttle(
                &root_password,
                config.auth.session_ttl(),
                config.auth.to_throttle(),
            )?,
        )
        .with_shutdown_state(shutdown_state.clone()),
    );
    graph_service.hydrate_persisted_users().await?;
    drop(root_password);

    // Both listener sockets are already reserved. Any unexpected server exit
    // is supervised alongside OS signals and causes an unsuccessful shutdown.
    let graph_server =
        byoridb_graph::server::GraphServer::with_service(graph_listen_addr, graph_service.clone());
    let mut graph_shutdown_rx = shutdown_tx.subscribe();
    let mut graph_handle = tokio::spawn(async move {
        tokio::select! {
            result = graph_server.start_with_listener(graph_listener) =>
                result.map_err(|error| error.to_string()),
            _ = graph_shutdown_rx.recv() => Ok(()),
        }
    });
    let http_server =
        byoridb_graph::server::HttpServer::with_service(http_listen_addr, graph_service.clone());
    let mut http_shutdown_rx = shutdown_tx.subscribe();
    let mut http_handle = tokio::spawn(async move {
        tokio::select! {
            result = http_server.start_with_listener(http_listener) =>
                result.map_err(|error| error.to_string()),
            _ = http_shutdown_rx.recv() => Ok(()),
        }
    });

    info!(%graph_listen_addr, %http_listen_addr, "ByoriDB Server is ready");
    let mut failures = Vec::new();
    let mut graph_finished = false;
    let mut http_finished = false;
    tokio::select! {
        result = tokio::signal::ctrl_c() => {
            if let Err(error) = result {
                failures.push(format!("Ctrl+C handler failed: {error}"));
            }
            info!("Initiating graceful shutdown");
        }
        _ = terminate_signal() => info!("Received SIGTERM, initiating graceful shutdown"),
        result = &mut graph_handle => {
            graph_finished = true;
            failures.push(format!("gRPC service exited unexpectedly: {result:?}"));
        }
        result = &mut http_handle => {
            http_finished = true;
            failures.push(format!("HTTP service exited unexpectedly: {result:?}"));
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
        failures
            .push("active query drain timed out; mutation outcomes may be uncertain".to_string());
        warn!(
            "Drain timed out after {:?}; {} query(ies) still running will be cut",
            drain_timeout,
            shutdown_state.active_queries()
        );
    }

    info!("Shutting down services...");

    // Send shutdown signal to all services
    let _ = shutdown_tx.send(());

    for (name, finished, handle) in [
        ("gRPC", graph_finished, &mut graph_handle),
        ("HTTP", http_finished, &mut http_handle),
    ] {
        if !finished {
            if let Err(error) = finish_service(name, handle).await {
                failures.push(error.to_string());
            }
        }
    }

    // Stop storage server. This checkpoints redb (2-phase Immediate commit) so
    // the next startup finds a clean shutdown and skips the full-repair scan,
    // which on a large dataset takes many minutes (2026-06-26 incident).
    info!("Stopping Storage Server...");
    if let Err(e) = storage_server.stop().await {
        failures.push(format!("Storage checkpoint failed: {e}"));
    }

    anyhow::ensure!(
        failures.is_empty(),
        "Server shutdown failed: {}",
        failures.join("; ")
    );
    info!("ByoriDB Server shutdown complete");
    Ok(())
}

async fn finish_service(
    name: &str,
    handle: &mut tokio::task::JoinHandle<Result<(), String>>,
) -> anyhow::Result<()> {
    match tokio::time::timeout(std::time::Duration::from_secs(30), &mut *handle).await {
        Ok(result) => result
            .with_context(|| format!("{name} server task failed"))?
            .map_err(|error| anyhow::anyhow!("{name} server failed: {error}")),
        Err(_) => {
            handle.abort();
            let _ = handle.await;
            anyhow::bail!("{name} server shutdown timed out");
        }
    }
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

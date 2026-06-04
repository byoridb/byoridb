// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Heartbeat sender for storage node registration
//!
//! This module provides a background task that periodically sends heartbeats
//! to the Meta service to register the storage node as available for
//! partition allocation.

use byoridb_meta::MetaClient;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

/// Heartbeat sender configuration
#[derive(Debug, Clone)]
pub struct HeartbeatConfig {
    /// Meta service address
    pub meta_addr: String,
    /// This node's hostname
    pub local_host: String,
    /// This node's port
    pub local_port: u32,
    /// Heartbeat interval
    pub interval: Duration,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            meta_addr: "localhost:9559".to_string(),
            local_host: "localhost".to_string(),
            local_port: 9779,
            interval: Duration::from_secs(10),
        }
    }
}

/// Heartbeat sender that runs as a background task
pub struct HeartbeatSender {
    config: HeartbeatConfig,
    shutdown: watch::Receiver<bool>,
}

impl HeartbeatSender {
    /// Create a new heartbeat sender
    pub fn new(config: HeartbeatConfig, shutdown: watch::Receiver<bool>) -> Self {
        Self { config, shutdown }
    }

    /// Run the heartbeat sender
    ///
    /// This will continuously send heartbeats until the shutdown signal is received.
    pub async fn run(mut self) {
        info!(
            "Starting heartbeat sender to {} as {}:{}",
            self.config.meta_addr, self.config.local_host, self.config.local_port
        );

        // Connect to Meta service
        let meta_client = match MetaClient::new(&self.config.meta_addr).await {
            Ok(client) => Arc::new(client),
            Err(e) => {
                error!(
                    "Failed to connect to Meta service at {}: {}",
                    self.config.meta_addr, e
                );
                return;
            }
        };

        info!("Connected to Meta service at {}", self.config.meta_addr);

        let mut interval = tokio::time::interval(self.config.interval);
        let mut consecutive_failures = 0u32;
        const MAX_FAILURES_BEFORE_WARN: u32 = 3;

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    match meta_client.send_heartbeat(
                        &self.config.local_host,
                        self.config.local_port,
                        "storage",
                    ).await {
                        Ok(cluster_id) => {
                            if consecutive_failures > 0 {
                                info!("Heartbeat recovered after {} failures, cluster_id: {}",
                                      consecutive_failures, cluster_id);
                            } else {
                                debug!("Heartbeat sent successfully, cluster_id: {}", cluster_id);
                            }
                            consecutive_failures = 0;
                        }
                        Err(e) => {
                            consecutive_failures += 1;
                            if consecutive_failures >= MAX_FAILURES_BEFORE_WARN {
                                warn!(
                                    "Failed to send heartbeat ({} consecutive failures): {}",
                                    consecutive_failures, e
                                );
                            } else {
                                debug!("Failed to send heartbeat: {}", e);
                            }
                        }
                    }
                }
                _ = self.shutdown.changed() => {
                    if *self.shutdown.borrow() {
                        info!("Heartbeat sender shutting down");
                        break;
                    }
                }
            }
        }
    }
}

/// Spawn a heartbeat sender as a background task
///
/// Returns:
/// - `JoinHandle` for the background task
/// - `Sender` to signal shutdown
///
/// # Example
/// ```ignore
/// let config = HeartbeatConfig {
///     meta_addr: "localhost:9559".to_string(),
///     local_host: "localhost".to_string(),
///     local_port: 9779,
///     interval: Duration::from_secs(10),
/// };
///
/// let (handle, shutdown_tx) = spawn_heartbeat_sender(config);
///
/// // Later, to stop the heartbeat sender:
/// let _ = shutdown_tx.send(true);
/// handle.await;
/// ```
pub fn spawn_heartbeat_sender(
    config: HeartbeatConfig,
) -> (tokio::task::JoinHandle<()>, watch::Sender<bool>) {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let sender = HeartbeatSender::new(config, shutdown_rx);

    let handle = tokio::spawn(async move {
        sender.run().await;
    });

    (handle, shutdown_tx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = HeartbeatConfig::default();
        assert_eq!(config.meta_addr, "localhost:9559");
        assert_eq!(config.local_host, "localhost");
        assert_eq!(config.local_port, 9779);
        assert_eq!(config.interval, Duration::from_secs(10));
    }
}

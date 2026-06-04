// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Raft consensus driver
//!
//! This module provides the main entry point for running Raft consensus.
//! It integrates the RaftGroupManager with the network layer and handles
//! periodic ticking and action processing.

use super::network::{RaftNetworkClient, RaftNetworkError, RaftNetworkService};
use super::node::RaftAction;
use super::types::{ClusterConfig, Command, NodeId};
use super::RaftGroupManager;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tonic::transport::Server;
use tracing::{debug, error, info};

/// Configuration for the Raft driver
#[derive(Debug, Clone)]
pub struct RaftDriverConfig {
    /// The node ID for this storage node
    pub node_id: NodeId,
    /// Address to bind the Raft RPC server
    pub bind_addr: SocketAddr,
    /// Cluster configuration
    pub cluster_config: ClusterConfig,
    /// Tick interval for driving Raft state machines
    pub tick_interval: Duration,
}

impl Default for RaftDriverConfig {
    fn default() -> Self {
        Self {
            node_id: 1,
            bind_addr: "0.0.0.0:9779".parse().unwrap(),
            cluster_config: ClusterConfig::default(),
            tick_interval: Duration::from_millis(50),
        }
    }
}

/// Callback for applying committed entries to the state machine
pub type ApplyCallback = Arc<dyn Fn(u32, u32, Command) + Send + Sync>;

/// The main Raft consensus driver
pub struct RaftDriver {
    /// Configuration
    config: RaftDriverConfig,
    /// The Raft group manager
    manager: Arc<RaftGroupManager>,
    /// Network client for sending RPCs
    client: Arc<RaftNetworkClient>,
    /// Callback for applying entries
    apply_callback: Option<ApplyCallback>,
    /// Shutdown signal sender
    shutdown_tx: Option<mpsc::Sender<()>>,
    /// Server task handle
    server_handle: Option<JoinHandle<()>>,
    /// Tick task handle
    tick_handle: Option<JoinHandle<()>>,
}

impl RaftDriver {
    /// Create a new Raft driver
    pub fn new(config: RaftDriverConfig) -> Self {
        let manager = Arc::new(RaftGroupManager::new(
            config.node_id,
            config.cluster_config.clone(),
        ));

        let client = Arc::new(RaftNetworkClient::new(config.node_id));

        Self {
            config,
            manager,
            client,
            apply_callback: None,
            shutdown_tx: None,
            server_handle: None,
            tick_handle: None,
        }
    }

    /// Set the callback for applying committed entries
    pub fn set_apply_callback(&mut self, callback: ApplyCallback) {
        self.apply_callback = Some(callback);
    }

    /// Get a reference to the Raft group manager
    pub fn manager(&self) -> Arc<RaftGroupManager> {
        Arc::clone(&self.manager)
    }

    /// Get a reference to the network client
    pub fn client(&self) -> Arc<RaftNetworkClient> {
        Arc::clone(&self.client)
    }

    /// Start the Raft driver
    pub async fn start(&mut self) -> Result<(), RaftDriverError> {
        info!(
            "Starting Raft driver for node {} at {}",
            self.config.node_id, self.config.bind_addr
        );

        // Initialize network client with cluster nodes
        for node in self.config.cluster_config.nodes.values() {
            if node.id != self.config.node_id {
                let addr = format!("http://{}:{}", node.addr, node.port);
                self.client.add_node(node.id, addr).await;
            }
        }

        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);
        self.shutdown_tx = Some(shutdown_tx);

        // Start the gRPC server
        let service = RaftNetworkService::new(Arc::clone(&self.manager));
        let addr = self.config.bind_addr;

        let server_handle = tokio::spawn(async move {
            info!("Starting Raft RPC server on {}", addr);
            if let Err(e) = Server::builder()
                .add_service(service.into_server())
                .serve(addr)
                .await
            {
                error!("Raft RPC server failed: {}", e);
            }
        });
        self.server_handle = Some(server_handle);

        // Start the tick task
        let manager = Arc::clone(&self.manager);
        let client = Arc::clone(&self.client);
        let tick_interval = self.config.tick_interval;
        let apply_callback = self.apply_callback.clone();

        let tick_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(tick_interval);
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        Self::process_tick(&manager, &client, &apply_callback).await;
                    }
                    _ = shutdown_rx.recv() => {
                        info!("Raft tick task shutting down");
                        break;
                    }
                }
            }
        });
        self.tick_handle = Some(tick_handle);

        info!("Raft driver started successfully");
        Ok(())
    }

    /// Process a single tick for all Raft groups
    async fn process_tick(
        manager: &Arc<RaftGroupManager>,
        client: &Arc<RaftNetworkClient>,
        apply_callback: &Option<ApplyCallback>,
    ) {
        let actions = manager.tick_all().await;

        for ((space_id, part_id), action) in actions {
            match action {
                RaftAction::None => {}
                RaftAction::SendRequestVote(requests) => {
                    for (node_id, request) in requests {
                        let client = Arc::clone(client);
                        let manager = Arc::clone(manager);
                        tokio::spawn(async move {
                            match client
                                .send_request_vote(node_id, space_id, part_id, request)
                                .await
                            {
                                Ok(response) => {
                                    // Process the response
                                    if let Some(group) = manager.get_group(space_id, part_id).await
                                    {
                                        let mut node = group.write().await;
                                        node.handle_request_vote_response(node_id, response);
                                    }
                                }
                                Err(e) => {
                                    debug!("RequestVote to node {} failed: {}", node_id, e);
                                }
                            }
                        });
                    }
                }
                RaftAction::SendAppendEntries(requests) => {
                    for (node_id, request) in requests {
                        let client = Arc::clone(client);
                        let manager = Arc::clone(manager);
                        tokio::spawn(async move {
                            match client
                                .send_append_entries(node_id, space_id, part_id, request)
                                .await
                            {
                                Ok(response) => {
                                    // Process the response
                                    if let Some(group) = manager.get_group(space_id, part_id).await
                                    {
                                        let mut node = group.write().await;
                                        node.handle_append_entries_response(node_id, response);
                                    }
                                }
                                Err(e) => {
                                    debug!("AppendEntries to node {} failed: {}", node_id, e);
                                }
                            }
                        });
                    }
                }
                RaftAction::ApplyEntries(entries) => {
                    if let Some(callback) = apply_callback {
                        for entry in entries {
                            callback(space_id, part_id, entry.command);
                        }
                    }
                }
            }
        }
    }

    /// Propose a command to a partition's Raft group
    pub async fn propose(
        &self,
        space_id: u32,
        part_id: u32,
        command: Command,
    ) -> Result<u64, RaftDriverError> {
        self.manager
            .propose(space_id, part_id, command)
            .await
            .map_err(|e| RaftDriverError::ProposeFailed(e.to_string()))
    }

    /// Check if this node is the leader for a partition
    pub async fn is_leader(&self, space_id: u32, part_id: u32) -> bool {
        self.manager.is_leader(space_id, part_id).await
    }

    /// Get the leader for a partition
    pub async fn get_leader(&self, space_id: u32, part_id: u32) -> Option<NodeId> {
        self.manager.get_leader(space_id, part_id).await
    }

    /// Shutdown the Raft driver
    pub async fn shutdown(&mut self) {
        info!("Shutting down Raft driver");

        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(()).await;
        }

        if let Some(handle) = self.tick_handle.take() {
            let _ = handle.await;
        }

        // Note: The server handle doesn't have a graceful shutdown in this simple impl
        // In production, you'd want to use Server::serve_with_shutdown
        if let Some(handle) = self.server_handle.take() {
            handle.abort();
        }

        info!("Raft driver shut down");
    }
}

/// Errors from the Raft driver
#[derive(Debug, thiserror::Error)]
pub enum RaftDriverError {
    #[error("Failed to start server: {0}")]
    ServerStart(String),

    #[error("Failed to propose command: {0}")]
    ProposeFailed(String),

    #[error("Network error: {0}")]
    Network(#[from] RaftNetworkError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raft::NodeInfo;

    fn create_test_config() -> RaftDriverConfig {
        let mut cluster_config = ClusterConfig::default();
        cluster_config.add_node(NodeInfo {
            id: 1,
            addr: "localhost".to_string(),
            port: 19779,
        });

        RaftDriverConfig {
            node_id: 1,
            bind_addr: "127.0.0.1:19779".parse().unwrap(),
            cluster_config,
            tick_interval: Duration::from_millis(50),
        }
    }

    #[tokio::test]
    async fn test_driver_creation() {
        let config = create_test_config();
        let driver = RaftDriver::new(config);

        assert!(!driver.is_leader(1, 1).await);
    }
}

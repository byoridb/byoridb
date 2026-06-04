// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Raft network layer
//!
//! This module provides gRPC-based network communication for the Raft consensus protocol.
//! It includes both the server (service) and client implementations.

use super::types::{
    AppendEntriesRequest as RaftAppendEntriesRequest,
    AppendEntriesResponse as RaftAppendEntriesResponse, Command,
    InstallSnapshotRequest as RaftInstallSnapshotRequest,
    InstallSnapshotResponse as RaftInstallSnapshotResponse, LogEntry as RaftLogEntry, NodeId,
    NodeInfo, RequestVoteRequest as RaftRequestVoteRequest,
    RequestVoteResponse as RaftRequestVoteResponse,
};
use super::RaftGroupManager;

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tonic::{Request, Response, Status};
use tracing::{debug, error, info, warn};

// Include the generated proto code
pub mod proto {
    tonic::include_proto!("raft");
}

use proto::raft_service_client::RaftServiceClient;
use proto::raft_service_server::{RaftService, RaftServiceServer};

/// Raft network service - handles incoming RPC requests
pub struct RaftNetworkService {
    /// The Raft group manager
    manager: Arc<RaftGroupManager>,
}

impl RaftNetworkService {
    pub fn new(manager: Arc<RaftGroupManager>) -> Self {
        Self { manager }
    }

    /// Create a gRPC server for this service
    pub fn into_server(self) -> RaftServiceServer<Self> {
        RaftServiceServer::new(self)
    }
}

#[tonic::async_trait]
impl RaftService for RaftNetworkService {
    async fn request_vote(
        &self,
        request: Request<proto::RequestVoteRequest>,
    ) -> Result<Response<proto::RequestVoteResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "Received RequestVote for space={}, part={}, term={}, candidate={}",
            req.space_id, req.part_id, req.term, req.candidate_id
        );

        // Convert proto request to internal type
        let internal_req = RaftRequestVoteRequest {
            term: req.term,
            candidate_id: req.candidate_id,
            last_log_index: req.last_log_index,
            last_log_term: req.last_log_term,
        };

        // Handle the request
        let result = self
            .manager
            .handle_request_vote(req.space_id, req.part_id, internal_req)
            .await;

        match result {
            Ok(resp) => Ok(Response::new(proto::RequestVoteResponse {
                term: resp.term,
                vote_granted: resp.vote_granted,
            })),
            Err(e) => {
                error!("RequestVote failed: {}", e);
                Err(Status::internal(e.to_string()))
            }
        }
    }

    async fn append_entries(
        &self,
        request: Request<proto::AppendEntriesRequest>,
    ) -> Result<Response<proto::AppendEntriesResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "Received AppendEntries for space={}, part={}, term={}, leader={}, entries={}",
            req.space_id,
            req.part_id,
            req.term,
            req.leader_id,
            req.entries.len()
        );

        // Convert proto entries to internal type
        let entries: Vec<RaftLogEntry> = req
            .entries
            .into_iter()
            .filter_map(|e| {
                // Deserialize command from bytes
                match bincode::deserialize::<Command>(&e.command) {
                    Ok(cmd) => Some(RaftLogEntry {
                        term: e.term,
                        index: e.index,
                        command: cmd,
                    }),
                    Err(err) => {
                        warn!("Failed to deserialize log entry command: {}", err);
                        None
                    }
                }
            })
            .collect();

        let internal_req = RaftAppendEntriesRequest {
            term: req.term,
            leader_id: req.leader_id,
            prev_log_index: req.prev_log_index,
            prev_log_term: req.prev_log_term,
            entries,
            leader_commit: req.leader_commit,
        };

        let result = self
            .manager
            .handle_append_entries(req.space_id, req.part_id, internal_req)
            .await;

        match result {
            Ok(resp) => Ok(Response::new(proto::AppendEntriesResponse {
                term: resp.term,
                success: resp.success,
                match_index: resp.match_index,
            })),
            Err(e) => {
                error!("AppendEntries failed: {}", e);
                Err(Status::internal(e.to_string()))
            }
        }
    }

    async fn install_snapshot(
        &self,
        request: Request<proto::InstallSnapshotRequest>,
    ) -> Result<Response<proto::InstallSnapshotResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "Received InstallSnapshot for space={}, part={}, term={}, leader={}, offset={}, done={}",
            req.space_id, req.part_id, req.term, req.leader_id, req.offset, req.done
        );

        // Convert proto request to internal type
        let internal_req = RaftInstallSnapshotRequest {
            term: req.term,
            leader_id: req.leader_id,
            last_included_index: req.last_included_index,
            last_included_term: req.last_included_term,
            offset: req.offset,
            data: req.data,
            done: req.done,
        };

        // Handle the request through RaftGroupManager
        let result = self
            .manager
            .handle_install_snapshot(req.space_id, req.part_id, internal_req)
            .await;

        match result {
            Ok((resp, action_opt)) => {
                // If there's an action, the snapshot chunk needs to be processed
                if let Some(action) = action_opt {
                    info!(
                        "InstallSnapshot action: last_index={}, last_term={}, offset={}, data_len={}, done={}",
                        action.last_included_index,
                        action.last_included_term,
                        action.offset,
                        action.data.len(),
                        action.done
                    );

                    // If this is the final chunk, apply the snapshot to the Raft node
                    if action.done {
                        let group = self
                            .manager
                            .get_or_create_group(req.space_id, req.part_id)
                            .await;
                        let mut node = group.write().await;
                        node.apply_snapshot(action.last_included_index, action.last_included_term);
                        info!(
                            "Applied snapshot for space={}, part={}: index={}, term={}",
                            req.space_id,
                            req.part_id,
                            action.last_included_index,
                            action.last_included_term
                        );
                    }
                }

                Ok(Response::new(proto::InstallSnapshotResponse {
                    term: resp.term,
                }))
            }
            Err(e) => {
                error!("InstallSnapshot failed: {}", e);
                Err(Status::internal(e.to_string()))
            }
        }
    }
}

/// Raft network client - sends RPC requests to other nodes
pub struct RaftNetworkClient {
    /// Node ID of this client (used for debugging and future features)
    #[allow(dead_code)]
    node_id: NodeId,
    /// Connected clients indexed by node ID
    clients: RwLock<HashMap<NodeId, RaftServiceClient<tonic::transport::Channel>>>,
    /// Node addresses for connection
    node_addrs: RwLock<HashMap<NodeId, String>>,
}

impl RaftNetworkClient {
    pub fn new(node_id: NodeId) -> Self {
        Self {
            node_id,
            clients: RwLock::new(HashMap::new()),
            node_addrs: RwLock::new(HashMap::new()),
        }
    }

    /// Add or update a node's address
    pub async fn add_node(&self, node_id: NodeId, addr: String) {
        let mut addrs = self.node_addrs.write().await;
        addrs.insert(node_id, addr);
    }

    /// Add multiple nodes from NodeInfo
    pub async fn add_nodes(&self, nodes: impl IntoIterator<Item = &NodeInfo>) {
        let mut addrs = self.node_addrs.write().await;
        for node in nodes {
            let addr = format!("http://{}:{}", node.addr, node.port);
            addrs.insert(node.id, addr);
        }
    }

    /// Remove a node
    pub async fn remove_node(&self, node_id: NodeId) {
        let mut addrs = self.node_addrs.write().await;
        addrs.remove(&node_id);

        let mut clients = self.clients.write().await;
        clients.remove(&node_id);
    }

    /// Get or create a client connection to a node
    async fn get_client(
        &self,
        node_id: NodeId,
    ) -> Result<RaftServiceClient<tonic::transport::Channel>, RaftNetworkError> {
        // Check if we already have a connection
        {
            let clients = self.clients.read().await;
            if let Some(client) = clients.get(&node_id) {
                return Ok(client.clone());
            }
        }

        // Get the address for this node
        let addr = {
            let addrs = self.node_addrs.read().await;
            addrs
                .get(&node_id)
                .cloned()
                .ok_or(RaftNetworkError::UnknownNode(node_id))?
        };

        // Create a new connection
        let client = RaftServiceClient::connect(addr.clone())
            .await
            .map_err(|e| RaftNetworkError::ConnectionFailed(node_id, e.to_string()))?;

        // Cache the connection
        let mut clients = self.clients.write().await;
        clients.insert(node_id, client.clone());

        info!("Connected to node {} at {}", node_id, addr);
        Ok(client)
    }

    /// Send RequestVote RPC to a node
    pub async fn send_request_vote(
        &self,
        target: NodeId,
        space_id: u32,
        part_id: u32,
        request: RaftRequestVoteRequest,
    ) -> Result<RaftRequestVoteResponse, RaftNetworkError> {
        let mut client = self.get_client(target).await?;

        let proto_req = proto::RequestVoteRequest {
            space_id,
            part_id,
            term: request.term,
            candidate_id: request.candidate_id,
            last_log_index: request.last_log_index,
            last_log_term: request.last_log_term,
        };

        let response = client
            .request_vote(proto_req)
            .await
            .map_err(|e| RaftNetworkError::RpcFailed(target, e.to_string()))?;

        let resp = response.into_inner();
        Ok(RaftRequestVoteResponse {
            term: resp.term,
            vote_granted: resp.vote_granted,
        })
    }

    /// Send AppendEntries RPC to a node
    pub async fn send_append_entries(
        &self,
        target: NodeId,
        space_id: u32,
        part_id: u32,
        request: RaftAppendEntriesRequest,
    ) -> Result<RaftAppendEntriesResponse, RaftNetworkError> {
        let mut client = self.get_client(target).await?;

        // Convert entries to proto format
        let entries: Vec<proto::LogEntry> = request
            .entries
            .into_iter()
            .map(|e| {
                let command_bytes = bincode::serialize(&e.command).unwrap_or_default();
                proto::LogEntry {
                    term: e.term,
                    index: e.index,
                    command: command_bytes,
                }
            })
            .collect();

        let proto_req = proto::AppendEntriesRequest {
            space_id,
            part_id,
            term: request.term,
            leader_id: request.leader_id,
            prev_log_index: request.prev_log_index,
            prev_log_term: request.prev_log_term,
            entries,
            leader_commit: request.leader_commit,
        };

        let response = client
            .append_entries(proto_req)
            .await
            .map_err(|e| RaftNetworkError::RpcFailed(target, e.to_string()))?;

        let resp = response.into_inner();
        Ok(RaftAppendEntriesResponse {
            term: resp.term,
            success: resp.success,
            match_index: resp.match_index,
        })
    }

    /// Send InstallSnapshot RPC to a node
    pub async fn send_install_snapshot(
        &self,
        target: NodeId,
        space_id: u32,
        part_id: u32,
        request: RaftInstallSnapshotRequest,
    ) -> Result<RaftInstallSnapshotResponse, RaftNetworkError> {
        let mut client = self.get_client(target).await?;

        let proto_req = proto::InstallSnapshotRequest {
            space_id,
            part_id,
            term: request.term,
            leader_id: request.leader_id,
            last_included_index: request.last_included_index,
            last_included_term: request.last_included_term,
            offset: request.offset,
            data: request.data,
            done: request.done,
        };

        let response = client
            .install_snapshot(proto_req)
            .await
            .map_err(|e| RaftNetworkError::RpcFailed(target, e.to_string()))?;

        let resp = response.into_inner();
        Ok(RaftInstallSnapshotResponse { term: resp.term })
    }
}

/// Errors from network operations
#[derive(Debug, thiserror::Error)]
pub enum RaftNetworkError {
    #[error("Unknown node: {0}")]
    UnknownNode(NodeId),

    #[error("Connection to node {0} failed: {1}")]
    ConnectionFailed(NodeId, String),

    #[error("RPC to node {0} failed: {1}")]
    RpcFailed(NodeId, String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raft::ClusterConfig;

    #[tokio::test]
    async fn test_network_client_creation() {
        let client = RaftNetworkClient::new(1);

        // Add some nodes
        client
            .add_node(2, "http://localhost:9780".to_string())
            .await;
        client
            .add_node(3, "http://localhost:9781".to_string())
            .await;

        // Verify nodes are stored
        let addrs = client.node_addrs.read().await;
        assert_eq!(addrs.len(), 2);
        assert_eq!(addrs.get(&2), Some(&"http://localhost:9780".to_string()));
    }

    #[tokio::test]
    async fn test_add_nodes_from_info() {
        let client = RaftNetworkClient::new(1);

        let nodes = vec![
            NodeInfo {
                id: 2,
                addr: "localhost".to_string(),
                port: 9780,
            },
            NodeInfo {
                id: 3,
                addr: "localhost".to_string(),
                port: 9781,
            },
        ];

        client.add_nodes(nodes.iter()).await;

        let addrs = client.node_addrs.read().await;
        assert_eq!(addrs.len(), 2);
        assert_eq!(addrs.get(&2), Some(&"http://localhost:9780".to_string()));
    }
}

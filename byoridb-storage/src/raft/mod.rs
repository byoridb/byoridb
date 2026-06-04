// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Raft consensus protocol implementation
//!
//! This module provides a custom Raft implementation for distributed consensus
//! in ByoriDB's storage layer. Each partition has its own Raft group for
//! replication and fault tolerance.
//!
//! # Components
//!
//! - `types`: Core Raft types (Term, LogIndex, LogEntry, RPC messages)
//! - `log`: Persistent log storage for Raft entries
//! - `node`: The main Raft state machine
//! - `group`: Manager for multiple Raft groups (one per partition)

// `driver` and `network` carry the gRPC transport for inter-node Raft and pull
// in tonic (not wasm-compatible). The core Raft state machine (node, log,
// storage, snapshot, types) stays pure-Rust. Gate the transport behind
// `distributed`; embedded never runs a Raft group.
#[cfg(feature = "distributed")]
mod driver;
mod log;
#[cfg(feature = "distributed")]
pub mod network;
mod node;
mod snapshot;
mod storage;
mod types;

#[cfg(feature = "distributed")]
pub use driver::{ApplyCallback, RaftDriver, RaftDriverConfig, RaftDriverError};
pub use log::RaftLog;
#[cfg(feature = "distributed")]
pub use network::{RaftNetworkClient, RaftNetworkError, RaftNetworkService};
pub use node::{InstallSnapshotAction, RaftAction, RaftConfig, RaftNode, RaftNodeError};
pub use snapshot::{
    SnapshotChunk, SnapshotError, SnapshotInstaller, SnapshotMeta, SnapshotReader, SnapshotWriter,
};
pub use storage::{RaftPersistentState, RaftStorage, RaftStorageError};
pub use types::{
    AppendEntriesRequest, AppendEntriesResponse, ClusterConfig, Command, ConfigChange,
    InstallSnapshotRequest, InstallSnapshotResponse, LogEntry, LogIndex, NodeId, NodeInfo,
    RaftMessage, RaftState, RequestVoteRequest, RequestVoteResponse, Term,
};

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Type alias for Raft group map
type RaftGroupMap = HashMap<(u32, u32), Arc<RwLock<RaftNode>>>;

/// Manages multiple Raft groups, one per partition
pub struct RaftGroupManager {
    /// Node ID for this storage node
    node_id: NodeId,
    /// Raft groups indexed by (space_id, part_id)
    groups: RwLock<RaftGroupMap>,
    /// Default cluster configuration
    default_config: ClusterConfig,
    /// Raft configuration
    raft_config: RaftConfig,
}

impl RaftGroupManager {
    /// Create a new RaftGroupManager
    pub fn new(node_id: NodeId, config: ClusterConfig) -> Self {
        info!("Creating RaftGroupManager for node {}", node_id);
        Self {
            node_id,
            groups: RwLock::new(HashMap::new()),
            default_config: config,
            raft_config: RaftConfig::default(),
        }
    }

    /// Create with custom Raft configuration
    pub fn with_config(node_id: NodeId, config: ClusterConfig, raft_config: RaftConfig) -> Self {
        info!(
            "Creating RaftGroupManager for node {} with custom config",
            node_id
        );
        Self {
            node_id,
            groups: RwLock::new(HashMap::new()),
            default_config: config,
            raft_config,
        }
    }

    /// Get or create a Raft group for a partition
    pub async fn get_or_create_group(&self, space_id: u32, part_id: u32) -> Arc<RwLock<RaftNode>> {
        let key = (space_id, part_id);

        // Try to get existing group
        {
            let groups = self.groups.read().await;
            if let Some(group) = groups.get(&key) {
                return Arc::clone(group);
            }
        }

        // Create new group
        let mut groups = self.groups.write().await;

        // Double-check after acquiring write lock
        if let Some(group) = groups.get(&key) {
            return Arc::clone(group);
        }

        debug!(
            "Creating Raft group for space={}, partition={}",
            space_id, part_id
        );

        let node = RaftNode::new(
            self.node_id,
            space_id,
            part_id,
            self.default_config.clone(),
            self.raft_config.clone(),
        );

        let group = Arc::new(RwLock::new(node));
        groups.insert(key, Arc::clone(&group));
        group
    }

    /// Get an existing Raft group
    pub async fn get_group(&self, space_id: u32, part_id: u32) -> Option<Arc<RwLock<RaftNode>>> {
        let groups = self.groups.read().await;
        groups.get(&(space_id, part_id)).cloned()
    }

    /// Remove a Raft group
    pub async fn remove_group(&self, space_id: u32, part_id: u32) -> Option<Arc<RwLock<RaftNode>>> {
        let mut groups = self.groups.write().await;
        groups.remove(&(space_id, part_id))
    }

    /// Propose a command to a partition's Raft group
    pub async fn propose(
        &self,
        space_id: u32,
        part_id: u32,
        command: Command,
    ) -> Result<LogIndex, RaftError> {
        let group = self
            .get_group(space_id, part_id)
            .await
            .ok_or(RaftError::GroupNotFound { space_id, part_id })?;

        let mut node = group.write().await;
        node.propose(command).map_err(|e| RaftError::NotLeader {
            leader_hint: e.to_string(),
        })
    }

    /// Check if this node is the leader for a partition
    pub async fn is_leader(&self, space_id: u32, part_id: u32) -> bool {
        if let Some(group) = self.get_group(space_id, part_id).await {
            let node = group.read().await;
            node.is_leader()
        } else {
            false
        }
    }

    /// Get leader ID for a partition
    pub async fn get_leader(&self, space_id: u32, part_id: u32) -> Option<NodeId> {
        if let Some(group) = self.get_group(space_id, part_id).await {
            let node = group.read().await;
            node.leader_id()
        } else {
            None
        }
    }

    /// Handle RequestVote RPC
    pub async fn handle_request_vote(
        &self,
        space_id: u32,
        part_id: u32,
        request: RequestVoteRequest,
    ) -> Result<RequestVoteResponse, RaftError> {
        let group = self.get_or_create_group(space_id, part_id).await;

        let mut node = group.write().await;
        Ok(node.handle_request_vote(request))
    }

    /// Handle AppendEntries RPC
    pub async fn handle_append_entries(
        &self,
        space_id: u32,
        part_id: u32,
        request: AppendEntriesRequest,
    ) -> Result<AppendEntriesResponse, RaftError> {
        let group = self.get_or_create_group(space_id, part_id).await;

        let mut node = group.write().await;
        Ok(node.handle_append_entries(request))
    }

    /// Handle InstallSnapshot RPC
    pub async fn handle_install_snapshot(
        &self,
        space_id: u32,
        part_id: u32,
        request: InstallSnapshotRequest,
    ) -> Result<(InstallSnapshotResponse, Option<InstallSnapshotAction>), RaftError> {
        let group = self.get_or_create_group(space_id, part_id).await;

        let mut node = group.write().await;
        Ok(node.handle_install_snapshot(request))
    }

    /// Propose a configuration change to add a node
    pub async fn propose_add_node(
        &self,
        space_id: u32,
        part_id: u32,
        node_id: NodeId,
        addr: String,
    ) -> Result<LogIndex, RaftError> {
        let group = self
            .get_group(space_id, part_id)
            .await
            .ok_or(RaftError::GroupNotFound { space_id, part_id })?;

        let mut node = group.write().await;
        node.propose_config_change(ConfigChange::AddNode { node_id, addr })
            .map_err(|e| RaftError::NotLeader {
                leader_hint: e.to_string(),
            })
    }

    /// Propose a configuration change to remove a node
    pub async fn propose_remove_node(
        &self,
        space_id: u32,
        part_id: u32,
        node_id: NodeId,
    ) -> Result<LogIndex, RaftError> {
        let group = self
            .get_group(space_id, part_id)
            .await
            .ok_or(RaftError::GroupNotFound { space_id, part_id })?;

        let mut node = group.write().await;
        node.propose_config_change(ConfigChange::RemoveNode { node_id })
            .map_err(|e| RaftError::NotLeader {
                leader_hint: e.to_string(),
            })
    }

    /// Tick all Raft groups (should be called periodically)
    /// Returns a list of actions that need to be processed
    pub async fn tick_all(&self) -> Vec<((u32, u32), RaftAction)> {
        let groups = self.groups.read().await;
        let mut actions = Vec::new();

        for ((space_id, part_id), group) in groups.iter() {
            let mut node = group.write().await;
            let action = node.tick();
            match &action {
                RaftAction::None => {}
                RaftAction::SendRequestVote(requests) => {
                    debug!(
                        "Raft tick: SendRequestVote to {} nodes for space={}, part={}",
                        requests.len(),
                        space_id,
                        part_id
                    );
                    actions.push(((*space_id, *part_id), action));
                }
                RaftAction::SendAppendEntries(requests) => {
                    debug!(
                        "Raft tick: SendAppendEntries to {} nodes for space={}, part={}",
                        requests.len(),
                        space_id,
                        part_id
                    );
                    actions.push(((*space_id, *part_id), action));
                }
                RaftAction::ApplyEntries(entries) => {
                    debug!(
                        "Raft tick: ApplyEntries {} entries for space={}, part={}",
                        entries.len(),
                        space_id,
                        part_id
                    );
                    actions.push(((*space_id, *part_id), action));
                }
            }
        }

        actions
    }

    /// Get all partition keys managed by this manager
    pub async fn get_all_partitions(&self) -> Vec<(u32, u32)> {
        let groups = self.groups.read().await;
        groups.keys().cloned().collect()
    }

    /// Get the number of Raft groups
    pub async fn group_count(&self) -> usize {
        let groups = self.groups.read().await;
        groups.len()
    }
}

/// Raft-related errors
#[derive(Debug, thiserror::Error)]
pub enum RaftError {
    #[error("Raft group not found for space={space_id}, partition={part_id}")]
    GroupNotFound { space_id: u32, part_id: u32 },

    #[error("Not leader, leader hint: {leader_hint}")]
    NotLeader { leader_hint: String },

    #[error("Log compacted, need snapshot")]
    LogCompacted,

    #[error("Storage error: {0}")]
    Storage(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> ClusterConfig {
        let mut config = ClusterConfig::new();
        config.add_node(NodeInfo {
            id: 1,
            addr: "localhost".to_string(),
            port: 9779,
        });
        config.add_node(NodeInfo {
            id: 2,
            addr: "localhost".to_string(),
            port: 9780,
        });
        config.add_node(NodeInfo {
            id: 3,
            addr: "localhost".to_string(),
            port: 9781,
        });
        config
    }

    #[tokio::test]
    async fn test_raft_group_manager_creation() {
        let config = create_test_config();
        let manager = RaftGroupManager::new(1, config);

        assert_eq!(manager.group_count().await, 0);
    }

    #[tokio::test]
    async fn test_get_or_create_group() {
        let config = create_test_config();
        let manager = RaftGroupManager::new(1, config);

        let _group1 = manager.get_or_create_group(1, 1).await;
        assert_eq!(manager.group_count().await, 1);

        // Getting the same group should return the same instance
        let _group2 = manager.get_or_create_group(1, 1).await;
        assert_eq!(manager.group_count().await, 1);

        // Different partition should create new group
        let _group3 = manager.get_or_create_group(1, 2).await;
        assert_eq!(manager.group_count().await, 2);
    }

    #[tokio::test]
    async fn test_remove_group() {
        let config = create_test_config();
        let manager = RaftGroupManager::new(1, config);

        manager.get_or_create_group(1, 1).await;
        manager.get_or_create_group(1, 2).await;
        assert_eq!(manager.group_count().await, 2);

        manager.remove_group(1, 1).await;
        assert_eq!(manager.group_count().await, 1);
        assert!(manager.get_group(1, 1).await.is_none());
        assert!(manager.get_group(1, 2).await.is_some());
    }

    #[tokio::test]
    async fn test_leader_operations() {
        let config = create_test_config();
        let manager = RaftGroupManager::new(1, config);

        // Initially not leader
        assert!(!manager.is_leader(1, 1).await);

        // Create group
        let group = manager.get_or_create_group(1, 1).await;

        // Still not leader (starts as follower)
        assert!(!manager.is_leader(1, 1).await);

        // Simulate becoming leader
        {
            let mut node = group.write().await;
            // Force become leader for testing
            node.become_candidate();
            // In real scenario, would need votes
        }
    }

    #[tokio::test]
    async fn test_get_all_partitions() {
        let config = create_test_config();
        let manager = RaftGroupManager::new(1, config);

        manager.get_or_create_group(1, 1).await;
        manager.get_or_create_group(1, 2).await;
        manager.get_or_create_group(2, 1).await;

        let partitions = manager.get_all_partitions().await;
        assert_eq!(partitions.len(), 3);
        assert!(partitions.contains(&(1, 1)));
        assert!(partitions.contains(&(1, 2)));
        assert!(partitions.contains(&(2, 1)));
    }

    /// Verify RaftError (group-level) and RaftNodeError (node-level) are distinct types
    /// and returned in the correct contexts.
    #[tokio::test]
    async fn test_raft_error_types_are_distinct() {
        // RaftError: group-level, returned by RaftGroupManager
        let group_err = RaftError::GroupNotFound {
            space_id: 1,
            part_id: 99,
        };
        assert!(group_err
            .to_string()
            .contains("Raft group not found for space=1, partition=99"));

        let leader_err = RaftError::NotLeader {
            leader_hint: "node 2".to_string(),
        };
        assert!(leader_err.to_string().contains("Not leader"));

        // RaftNodeError: node-level, returned by RaftNode::propose
        let node_err = RaftNodeError::NotLeader { leader_id: Some(2) };
        assert!(node_err
            .to_string()
            .contains("Not the leader (leader is Some(2))"));

        // Both types coexist without naming conflict
        let _: fn() -> RaftError = || RaftError::LogCompacted;
        let _: fn() -> RaftNodeError = || RaftNodeError::ProposalFailed("test".into());
    }

    /// Verify that proposing to a non-existent group returns RaftError::GroupNotFound
    #[tokio::test]
    async fn test_propose_to_missing_group_returns_group_error() {
        let config = create_test_config();
        let manager = RaftGroupManager::new(1, config);

        let result = manager
            .propose(
                99,
                99,
                Command::Put {
                    space_id: 99,
                    part_id: 99,
                    key: vec![],
                    value: vec![],
                },
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            err,
            RaftError::GroupNotFound {
                space_id: 99,
                part_id: 99
            }
        ));
    }

    /// Verify that proposing on a follower node returns RaftError::NotLeader
    /// (wrapping the underlying RaftNodeError)
    #[tokio::test]
    async fn test_propose_on_follower_returns_not_leader() {
        let config = create_test_config();
        let manager = RaftGroupManager::new(1, config);

        // Create group — node starts as follower
        manager.get_or_create_group(1, 1).await;

        let result = manager
            .propose(
                1,
                1,
                Command::Put {
                    space_id: 1,
                    part_id: 1,
                    key: vec![1],
                    value: vec![2],
                },
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, RaftError::NotLeader { .. }));
    }
}

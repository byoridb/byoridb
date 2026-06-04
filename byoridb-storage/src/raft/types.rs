// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Raft consensus types
//!
//! Core types for the Raft consensus protocol implementation.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Node identifier in the Raft cluster
pub type NodeId = u64;

/// Log index (1-based)
pub type LogIndex = u64;

/// Term number
pub type Term = u64;

/// Raft node state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RaftState {
    /// Follower state - receives log entries from leader
    #[default]
    Follower,
    /// Candidate state - requesting votes for leader election
    Candidate,
    /// Leader state - handles client requests and replicates logs
    Leader,
}

/// Raft log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    /// Term when entry was received by leader
    pub term: Term,
    /// Position in the log (1-indexed)
    pub index: LogIndex,
    /// Command to apply to state machine
    pub command: Command,
}

/// Command types that can be applied to the state machine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Command {
    /// No-op command (used for leader confirmation)
    Noop,
    /// Put a key-value pair
    Put {
        space_id: u32,
        part_id: u32,
        key: Vec<u8>,
        value: Vec<u8>,
    },
    /// Delete a key
    Delete {
        space_id: u32,
        part_id: u32,
        key: Vec<u8>,
    },
    /// Configuration change
    ConfigChange(ConfigChange),
}

/// Configuration change types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConfigChange {
    /// Add a new node to the cluster
    AddNode { node_id: NodeId, addr: String },
    /// Remove a node from the cluster
    RemoveNode { node_id: NodeId },
}

/// Node information in the cluster
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub id: NodeId,
    pub addr: String,
    pub port: u32,
}

/// Cluster configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClusterConfig {
    /// All nodes in the cluster
    pub nodes: HashMap<NodeId, NodeInfo>,
    /// Node IDs that can vote
    pub voters: Vec<NodeId>,
}

impl ClusterConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_node(&mut self, info: NodeInfo) {
        let id = info.id;
        self.nodes.insert(id, info);
        if !self.voters.contains(&id) {
            self.voters.push(id);
        }
    }

    pub fn remove_node(&mut self, node_id: NodeId) {
        self.nodes.remove(&node_id);
        self.voters.retain(|&id| id != node_id);
    }

    pub fn get_node(&self, node_id: NodeId) -> Option<&NodeInfo> {
        self.nodes.get(&node_id)
    }

    pub fn majority(&self) -> usize {
        self.voters.len() / 2 + 1
    }
}

/// RequestVote RPC request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestVoteRequest {
    /// Candidate's term
    pub term: Term,
    /// Candidate requesting vote
    pub candidate_id: NodeId,
    /// Index of candidate's last log entry
    pub last_log_index: LogIndex,
    /// Term of candidate's last log entry
    pub last_log_term: Term,
}

/// RequestVote RPC response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestVoteResponse {
    /// Current term, for candidate to update itself
    pub term: Term,
    /// True means candidate received vote
    pub vote_granted: bool,
}

/// AppendEntries RPC request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppendEntriesRequest {
    /// Leader's term
    pub term: Term,
    /// Leader's ID
    pub leader_id: NodeId,
    /// Index of log entry immediately preceding new ones
    pub prev_log_index: LogIndex,
    /// Term of prev_log_index entry
    pub prev_log_term: Term,
    /// Log entries to store (empty for heartbeat)
    pub entries: Vec<LogEntry>,
    /// Leader's commit index
    pub leader_commit: LogIndex,
}

/// AppendEntries RPC response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppendEntriesResponse {
    /// Current term, for leader to update itself
    pub term: Term,
    /// True if follower contained entry matching prev_log_index and prev_log_term
    pub success: bool,
    /// The index of the last log entry (for optimization)
    pub match_index: LogIndex,
}

/// InstallSnapshot RPC request (for catching up lagging followers)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallSnapshotRequest {
    /// Leader's term
    pub term: Term,
    /// Leader's ID
    pub leader_id: NodeId,
    /// The snapshot replaces all entries up through and including this index
    pub last_included_index: LogIndex,
    /// Term of last_included_index
    pub last_included_term: Term,
    /// Byte offset where chunk is positioned in the snapshot file
    pub offset: u64,
    /// Raw bytes of the snapshot chunk
    pub data: Vec<u8>,
    /// True if this is the last chunk
    pub done: bool,
}

/// InstallSnapshot RPC response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallSnapshotResponse {
    /// Current term, for leader to update itself
    pub term: Term,
}

/// Raft message envelope for network transport
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RaftMessage {
    RequestVote(RequestVoteRequest),
    RequestVoteResponse(RequestVoteResponse),
    AppendEntries(AppendEntriesRequest),
    AppendEntriesResponse(AppendEntriesResponse),
    InstallSnapshot(InstallSnapshotRequest),
    InstallSnapshotResponse(InstallSnapshotResponse),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cluster_config() {
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

        assert_eq!(config.nodes.len(), 3);
        assert_eq!(config.voters.len(), 3);
        assert_eq!(config.majority(), 2);

        config.remove_node(3);
        assert_eq!(config.nodes.len(), 2);
        assert_eq!(config.voters.len(), 2);
    }

    #[test]
    fn test_log_entry_serialization() {
        let entry = LogEntry {
            term: 1,
            index: 1,
            command: Command::Put {
                space_id: 1,
                part_id: 1,
                key: b"key".to_vec(),
                value: b"value".to_vec(),
            },
        };

        let serialized = serde_json::to_string(&entry).unwrap();
        let deserialized: LogEntry = serde_json::from_str(&serialized).unwrap();

        assert_eq!(entry.term, deserialized.term);
        assert_eq!(entry.index, deserialized.index);
    }
}

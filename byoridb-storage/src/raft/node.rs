// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Raft node implementation
//!
//! This module implements the core Raft consensus algorithm.

use super::log::RaftLog;
use super::types::*;
use rand::Rng;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tracing::{debug, info};

/// Raft configuration
#[derive(Debug, Clone)]
pub struct RaftConfig {
    /// Minimum election timeout in milliseconds
    pub election_timeout_min: u64,
    /// Maximum election timeout in milliseconds
    pub election_timeout_max: u64,
    /// Heartbeat interval in milliseconds
    pub heartbeat_interval: u64,
    /// Maximum entries per AppendEntries RPC
    pub max_entries_per_request: usize,
    /// Snapshot threshold (compact log when it exceeds this)
    pub snapshot_threshold: u64,
}

impl Default for RaftConfig {
    fn default() -> Self {
        Self {
            election_timeout_min: 150,
            election_timeout_max: 300,
            heartbeat_interval: 50,
            max_entries_per_request: 100,
            snapshot_threshold: 10000,
        }
    }
}

/// Persistent state on all servers
#[derive(Debug, Clone, Default)]
pub struct PersistentState {
    /// Latest term server has seen
    pub current_term: Term,
    /// CandidateId that received vote in current term (or None)
    pub voted_for: Option<NodeId>,
}

/// Volatile state on all servers
#[derive(Debug, Clone, Default)]
pub struct VolatileState {
    /// Index of highest log entry known to be committed
    pub commit_index: LogIndex,
    /// Index of highest log entry applied to state machine
    pub last_applied: LogIndex,
}

/// Volatile state on leaders (reinitialized after election)
#[derive(Debug, Clone, Default)]
pub struct LeaderState {
    /// For each server, index of the next log entry to send
    pub next_index: HashMap<NodeId, LogIndex>,
    /// For each server, index of highest log entry known to be replicated
    pub match_index: HashMap<NodeId, LogIndex>,
}

/// Raft node
#[allow(dead_code)]
pub struct RaftNode {
    /// This node's ID
    id: NodeId,
    /// Space and partition this node belongs to
    space_id: u32,
    part_id: u32,
    /// Current state (Follower, Candidate, Leader)
    state: RaftState,
    /// Persistent state
    persistent: PersistentState,
    /// Volatile state
    volatile: VolatileState,
    /// Leader-specific state
    leader_state: Option<LeaderState>,
    /// Raft log
    log: RaftLog,
    /// Cluster configuration
    config: ClusterConfig,
    /// Raft configuration
    raft_config: RaftConfig,
    /// Current leader ID (if known)
    leader_id: Option<NodeId>,
    /// Election timeout deadline
    election_deadline: Instant,
    /// Votes received in current election
    votes_received: HashMap<NodeId, bool>,
}

impl RaftNode {
    /// Create a new Raft node
    pub fn new(
        id: NodeId,
        space_id: u32,
        part_id: u32,
        config: ClusterConfig,
        raft_config: RaftConfig,
    ) -> Self {
        let log = RaftLog::new(space_id, part_id);
        let election_deadline = Self::random_election_deadline(&raft_config);

        Self {
            id,
            space_id,
            part_id,
            state: RaftState::Follower,
            persistent: PersistentState::default(),
            volatile: VolatileState::default(),
            leader_state: None,
            log,
            config,
            raft_config,
            leader_id: None,
            election_deadline,
            votes_received: HashMap::new(),
        }
    }

    /// Get node ID
    pub fn id(&self) -> NodeId {
        self.id
    }

    /// Get current state
    pub fn state(&self) -> RaftState {
        self.state
    }

    /// Get current term
    pub fn current_term(&self) -> Term {
        self.persistent.current_term
    }

    /// Get current leader ID
    pub fn leader_id(&self) -> Option<NodeId> {
        self.leader_id
    }

    /// Check if this node is the leader
    pub fn is_leader(&self) -> bool {
        self.state == RaftState::Leader
    }

    /// Get commit index
    pub fn commit_index(&self) -> LogIndex {
        self.volatile.commit_index
    }

    /// Generate a random election deadline
    fn random_election_deadline(config: &RaftConfig) -> Instant {
        let mut rng = rand::thread_rng();
        let timeout = rng.gen_range(config.election_timeout_min..=config.election_timeout_max);
        Instant::now() + Duration::from_millis(timeout)
    }

    /// Reset election timeout
    fn reset_election_timeout(&mut self) {
        self.election_deadline = Self::random_election_deadline(&self.raft_config);
    }

    /// Check if election timeout has elapsed
    pub fn election_timeout_elapsed(&self) -> bool {
        Instant::now() >= self.election_deadline
    }

    /// Become a follower
    fn become_follower(&mut self, term: Term, leader_id: Option<NodeId>) {
        info!(
            "Node {} becoming follower in term {} (leader: {:?})",
            self.id, term, leader_id
        );
        self.state = RaftState::Follower;
        self.persistent.current_term = term;
        self.persistent.voted_for = None;
        self.leader_id = leader_id;
        self.leader_state = None;
        self.reset_election_timeout();
    }

    /// Become a candidate and start election
    pub fn become_candidate(&mut self) -> Vec<(NodeId, RequestVoteRequest)> {
        self.persistent.current_term += 1;
        let term = self.persistent.current_term;

        info!("Node {} becoming candidate in term {}", self.id, term);

        self.state = RaftState::Candidate;
        self.persistent.voted_for = Some(self.id);
        self.leader_id = None;
        self.leader_state = None;
        self.votes_received.clear();
        self.votes_received.insert(self.id, true); // Vote for self
        self.reset_election_timeout();

        // Send RequestVote to all other nodes
        let request = RequestVoteRequest {
            term,
            candidate_id: self.id,
            last_log_index: self.log.last_index(),
            last_log_term: self.log.last_term(),
        };

        self.config
            .voters
            .iter()
            .filter(|&&node_id| node_id != self.id)
            .map(|&node_id| (node_id, request.clone()))
            .collect()
    }

    /// Become the leader
    fn become_leader(&mut self) {
        info!(
            "Node {} becoming leader in term {}",
            self.id, self.persistent.current_term
        );

        self.state = RaftState::Leader;
        self.leader_id = Some(self.id);

        // Initialize leader state
        let last_log_index = self.log.last_index();
        let mut leader_state = LeaderState::default();

        for &node_id in &self.config.voters {
            if node_id != self.id {
                leader_state.next_index.insert(node_id, last_log_index + 1);
                leader_state.match_index.insert(node_id, 0);
            }
        }

        self.leader_state = Some(leader_state);

        // Append a no-op entry to commit entries from previous terms
        self.log
            .append_entry(self.persistent.current_term, Command::Noop);
    }

    /// Handle RequestVote RPC
    pub fn handle_request_vote(&mut self, req: RequestVoteRequest) -> RequestVoteResponse {
        debug!(
            "Node {} received RequestVote from {} for term {}",
            self.id, req.candidate_id, req.term
        );

        // If term > currentTerm, update currentTerm and become follower
        if req.term > self.persistent.current_term {
            self.become_follower(req.term, None);
        }

        let vote_granted = if req.term < self.persistent.current_term {
            // Reject: candidate's term is stale
            false
        } else if self.persistent.voted_for.is_some()
            && self.persistent.voted_for != Some(req.candidate_id)
        {
            // Reject: already voted for someone else
            false
        } else if !self
            .log
            .is_up_to_date(req.last_log_index, req.last_log_term)
        {
            // Reject: candidate's log is not up-to-date
            false
        } else {
            // Grant vote
            self.persistent.voted_for = Some(req.candidate_id);
            self.reset_election_timeout();
            true
        };

        if vote_granted {
            info!(
                "Node {} granted vote to {} for term {}",
                self.id, req.candidate_id, req.term
            );
        }

        RequestVoteResponse {
            term: self.persistent.current_term,
            vote_granted,
        }
    }

    /// Handle RequestVote response
    pub fn handle_request_vote_response(
        &mut self,
        from: NodeId,
        resp: RequestVoteResponse,
    ) -> bool {
        // If not a candidate, ignore
        if self.state != RaftState::Candidate {
            return false;
        }

        // If response term > currentTerm, become follower
        if resp.term > self.persistent.current_term {
            self.become_follower(resp.term, None);
            return false;
        }

        // Record vote
        self.votes_received.insert(from, resp.vote_granted);

        // Check if we have majority
        let votes_for: usize = self.votes_received.values().filter(|&&v| v).count();
        if votes_for >= self.config.majority() {
            self.become_leader();
            return true;
        }

        false
    }

    /// Handle AppendEntries RPC
    pub fn handle_append_entries(&mut self, req: AppendEntriesRequest) -> AppendEntriesResponse {
        debug!(
            "Node {} received AppendEntries from {} (term={}, entries={})",
            self.id,
            req.leader_id,
            req.term,
            req.entries.len()
        );

        // If term > currentTerm, update currentTerm and become follower
        if req.term > self.persistent.current_term {
            self.become_follower(req.term, Some(req.leader_id));
        }

        // Reject if term < currentTerm
        if req.term < self.persistent.current_term {
            return AppendEntriesResponse {
                term: self.persistent.current_term,
                success: false,
                match_index: 0,
            };
        }

        // Reset election timeout (valid leader)
        self.reset_election_timeout();
        self.leader_id = Some(req.leader_id);

        // If candidate, step down to follower
        if self.state == RaftState::Candidate {
            self.become_follower(req.term, Some(req.leader_id));
        }

        // Check if log contains entry at prev_log_index with prev_log_term
        if req.prev_log_index > 0 {
            match self.log.term(req.prev_log_index) {
                None => {
                    // Log doesn't contain entry at prev_log_index
                    return AppendEntriesResponse {
                        term: self.persistent.current_term,
                        success: false,
                        match_index: self.log.last_index(),
                    };
                }
                Some(term) if term != req.prev_log_term => {
                    // Entry at prev_log_index has different term
                    // Delete conflicting entries
                    self.log.truncate(req.prev_log_index);
                    return AppendEntriesResponse {
                        term: self.persistent.current_term,
                        success: false,
                        match_index: self.log.last_index(),
                    };
                }
                _ => {}
            }
        }

        // Append new entries (if any)
        if !req.entries.is_empty() {
            // Find any conflicting entries and delete them
            for entry in &req.entries {
                if let Some(existing) = self.log.get(entry.index) {
                    if existing.term != entry.term {
                        self.log.truncate(entry.index);
                        break;
                    }
                }
            }

            // Append entries that don't exist yet
            let entries_to_append: Vec<LogEntry> = req
                .entries
                .into_iter()
                .filter(|e| e.index > self.log.last_index())
                .collect();

            if !entries_to_append.is_empty() {
                self.log.append(entries_to_append);
            }
        }

        // Update commit index
        if req.leader_commit > self.volatile.commit_index {
            self.volatile.commit_index = std::cmp::min(req.leader_commit, self.log.last_index());
        }

        AppendEntriesResponse {
            term: self.persistent.current_term,
            success: true,
            match_index: self.log.last_index(),
        }
    }

    /// Handle AppendEntries response (leader only)
    pub fn handle_append_entries_response(&mut self, from: NodeId, resp: AppendEntriesResponse) {
        // If not leader, ignore
        if self.state != RaftState::Leader {
            return;
        }

        // If response term > currentTerm, become follower
        if resp.term > self.persistent.current_term {
            self.become_follower(resp.term, None);
            return;
        }

        let leader_state = match &mut self.leader_state {
            Some(s) => s,
            None => return,
        };

        if resp.success {
            // Update match_index and next_index
            leader_state.match_index.insert(from, resp.match_index);
            leader_state.next_index.insert(from, resp.match_index + 1);

            // Try to advance commit index
            self.try_advance_commit_index();
        } else {
            // Decrement next_index and retry
            let next = leader_state.next_index.entry(from).or_insert(1);
            *next = next.saturating_sub(1).max(1);
        }
    }

    /// Try to advance the commit index (leader only)
    fn try_advance_commit_index(&mut self) {
        let leader_state = match &self.leader_state {
            Some(s) => s,
            None => return,
        };

        // Find the highest index that is replicated on a majority
        let mut match_indices: Vec<LogIndex> = leader_state.match_index.values().copied().collect();
        match_indices.push(self.log.last_index()); // Include leader's own index
        match_indices.sort_unstable();

        let majority_index = match_indices[match_indices.len() - self.config.majority()];

        // Only commit entries from current term
        if majority_index > self.volatile.commit_index {
            if let Some(term) = self.log.term(majority_index) {
                if term == self.persistent.current_term {
                    self.volatile.commit_index = majority_index;
                    debug!(
                        "Leader {} advanced commit index to {}",
                        self.id, majority_index
                    );
                }
            }
        }
    }

    /// Propose a new command (leader only)
    pub fn propose(&mut self, command: Command) -> Result<LogIndex, RaftNodeError> {
        if self.state != RaftState::Leader {
            return Err(RaftNodeError::NotLeader {
                leader_id: self.leader_id,
            });
        }

        let index = self.log.append_entry(self.persistent.current_term, command);
        Ok(index)
    }

    /// Generate heartbeat/append entries requests (leader only)
    pub fn generate_append_entries(&self) -> Vec<(NodeId, AppendEntriesRequest)> {
        if self.state != RaftState::Leader {
            return Vec::new();
        }

        let leader_state = match &self.leader_state {
            Some(s) => s,
            None => return Vec::new(),
        };

        let mut requests = Vec::new();

        for &node_id in &self.config.voters {
            if node_id == self.id {
                continue;
            }

            let next_index = *leader_state.next_index.get(&node_id).unwrap_or(&1);
            let prev_log_index = next_index.saturating_sub(1);
            let prev_log_term = self.log.term(prev_log_index).unwrap_or(0);

            let entries = self.log.entries_range(
                next_index,
                next_index + self.raft_config.max_entries_per_request as LogIndex,
            );

            let request = AppendEntriesRequest {
                term: self.persistent.current_term,
                leader_id: self.id,
                prev_log_index,
                prev_log_term,
                entries,
                leader_commit: self.volatile.commit_index,
            };

            requests.push((node_id, request));
        }

        requests
    }

    /// Get entries that can be applied to the state machine
    pub fn get_entries_to_apply(&mut self) -> Vec<LogEntry> {
        let mut entries = Vec::new();

        while self.volatile.last_applied < self.volatile.commit_index {
            self.volatile.last_applied += 1;
            if let Some(entry) = self.log.get(self.volatile.last_applied) {
                entries.push(entry.clone());
            }
        }

        entries
    }

    /// Handle InstallSnapshot RPC
    pub fn handle_install_snapshot(
        &mut self,
        req: super::types::InstallSnapshotRequest,
    ) -> (
        super::types::InstallSnapshotResponse,
        Option<InstallSnapshotAction>,
    ) {
        debug!(
            "Node {} received InstallSnapshot from {} (term={}, last_index={}, offset={})",
            self.id, req.leader_id, req.term, req.last_included_index, req.offset
        );

        // If term > currentTerm, update currentTerm and become follower
        if req.term > self.persistent.current_term {
            self.become_follower(req.term, Some(req.leader_id));
        }

        // Reply immediately if term < currentTerm
        if req.term < self.persistent.current_term {
            return (
                super::types::InstallSnapshotResponse {
                    term: self.persistent.current_term,
                },
                None,
            );
        }

        // Reset election timeout
        self.reset_election_timeout();
        self.leader_id = Some(req.leader_id);

        // Return action to install the snapshot chunk
        let action = InstallSnapshotAction {
            last_included_index: req.last_included_index,
            last_included_term: req.last_included_term,
            offset: req.offset,
            data: req.data,
            done: req.done,
        };

        (
            super::types::InstallSnapshotResponse {
                term: self.persistent.current_term,
            },
            Some(action),
        )
    }

    /// Apply a snapshot to this node's state
    pub fn apply_snapshot(&mut self, last_included_index: LogIndex, last_included_term: Term) {
        info!(
            "Node {} applying snapshot: index={}, term={}",
            self.id, last_included_index, last_included_term
        );

        // Discard log entries covered by the snapshot
        self.log.compact(last_included_index, last_included_term);

        // Update commit and applied indices
        if last_included_index > self.volatile.commit_index {
            self.volatile.commit_index = last_included_index;
        }
        if last_included_index > self.volatile.last_applied {
            self.volatile.last_applied = last_included_index;
        }
    }

    /// Check if this node should send a snapshot to a follower
    /// Returns true if the next_index for the follower is before our first log entry
    pub fn should_send_snapshot(&self, follower_id: NodeId) -> bool {
        if self.state != RaftState::Leader {
            return false;
        }

        let leader_state = match &self.leader_state {
            Some(s) => s,
            None => return false,
        };

        let next_index = *leader_state.next_index.get(&follower_id).unwrap_or(&1);
        let first_index = self.log.first_index();

        // If the follower needs entries we no longer have, send snapshot
        next_index < first_index
    }

    /// Propose a configuration change (add or remove node)
    pub fn propose_config_change(
        &mut self,
        change: ConfigChange,
    ) -> Result<LogIndex, RaftNodeError> {
        if self.state != RaftState::Leader {
            return Err(RaftNodeError::NotLeader {
                leader_id: self.leader_id,
            });
        }

        info!("Node {} proposing config change: {:?}", self.id, change);

        let index = self
            .log
            .append_entry(self.persistent.current_term, Command::ConfigChange(change));
        Ok(index)
    }

    /// Apply a configuration change
    pub fn apply_config_change(&mut self, change: &ConfigChange) {
        match change {
            ConfigChange::AddNode { node_id, addr } => {
                info!(
                    "Node {} adding node {} at {} to cluster",
                    self.id, node_id, addr
                );

                // Add to cluster config
                self.config.add_node(NodeInfo {
                    id: *node_id,
                    addr: addr.clone(),
                    port: self.extract_port(addr),
                });

                // If we're leader, initialize tracking for the new node
                if let Some(leader_state) = &mut self.leader_state {
                    let last_log_index = self.log.last_index();
                    leader_state.next_index.insert(*node_id, last_log_index + 1);
                    leader_state.match_index.insert(*node_id, 0);
                }
            }
            ConfigChange::RemoveNode { node_id } => {
                info!("Node {} removing node {} from cluster", self.id, node_id);

                // Remove from cluster config
                self.config.remove_node(*node_id);

                // If we're leader, stop tracking the removed node
                if let Some(leader_state) = &mut self.leader_state {
                    leader_state.next_index.remove(node_id);
                    leader_state.match_index.remove(node_id);
                }

                // If this node is being removed, step down if leader
                if *node_id == self.id && self.state == RaftState::Leader {
                    self.become_follower(self.persistent.current_term, None);
                }
            }
        }
    }

    /// Extract port from address string (e.g., "localhost:9779" -> 9779)
    fn extract_port(&self, addr: &str) -> u32 {
        addr.split(':')
            .next_back()
            .and_then(|s| s.parse().ok())
            .unwrap_or(9779)
    }

    /// Get the current cluster configuration
    pub fn cluster_config(&self) -> &ClusterConfig {
        &self.config
    }

    /// Tick - called periodically to drive the Raft state machine
    pub fn tick(&mut self) -> RaftAction {
        match self.state {
            RaftState::Follower | RaftState::Candidate => {
                if self.election_timeout_elapsed() {
                    let requests = self.become_candidate();
                    return RaftAction::SendRequestVote(requests);
                }
            }
            RaftState::Leader => {
                // Send heartbeats
                let requests = self.generate_append_entries();
                if !requests.is_empty() {
                    return RaftAction::SendAppendEntries(requests);
                }
            }
        }

        // Apply committed entries
        let entries = self.get_entries_to_apply();
        if !entries.is_empty() {
            return RaftAction::ApplyEntries(entries);
        }

        RaftAction::None
    }
}

/// Actions the Raft node wants to perform
#[derive(Debug)]
pub enum RaftAction {
    None,
    SendRequestVote(Vec<(NodeId, RequestVoteRequest)>),
    SendAppendEntries(Vec<(NodeId, AppendEntriesRequest)>),
    ApplyEntries(Vec<LogEntry>),
}

/// Action to install a snapshot chunk
#[derive(Debug, Clone)]
pub struct InstallSnapshotAction {
    pub last_included_index: LogIndex,
    pub last_included_term: Term,
    pub offset: u64,
    pub data: Vec<u8>,
    pub done: bool,
}

/// Raft node-level errors
#[derive(Debug, thiserror::Error)]
pub enum RaftNodeError {
    #[error("Not the leader (leader is {leader_id:?})")]
    NotLeader { leader_id: Option<NodeId> },

    #[error("Proposal failed: {0}")]
    ProposalFailed(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> ClusterConfig {
        let mut config = ClusterConfig::new();
        for i in 1..=3 {
            config.add_node(NodeInfo {
                id: i,
                addr: format!("localhost:{}", 9778 + i),
                port: 9778 + i as u32,
            });
        }
        config
    }

    #[test]
    fn test_initial_state() {
        let config = create_test_config();
        let node = RaftNode::new(1, 1, 1, config, RaftConfig::default());

        assert_eq!(node.state(), RaftState::Follower);
        assert_eq!(node.current_term(), 0);
        assert_eq!(node.leader_id(), None);
    }

    #[test]
    fn test_become_candidate() {
        let config = create_test_config();
        let mut node = RaftNode::new(1, 1, 1, config, RaftConfig::default());

        let requests = node.become_candidate();

        assert_eq!(node.state(), RaftState::Candidate);
        assert_eq!(node.current_term(), 1);
        assert_eq!(requests.len(), 2); // 2 other nodes
    }

    #[test]
    fn test_election_with_votes() {
        let config = create_test_config();
        let mut node = RaftNode::new(1, 1, 1, config, RaftConfig::default());

        node.become_candidate();

        // Receive vote from node 2
        let became_leader = node.handle_request_vote_response(
            2,
            RequestVoteResponse {
                term: 1,
                vote_granted: true,
            },
        );

        // With 2 votes (self + node 2), majority of 3 is achieved
        assert!(became_leader);
        assert_eq!(node.state(), RaftState::Leader);
    }

    #[test]
    fn test_append_entries_heartbeat() {
        let config = create_test_config();
        let mut follower = RaftNode::new(2, 1, 1, config.clone(), RaftConfig::default());

        let req = AppendEntriesRequest {
            term: 1,
            leader_id: 1,
            prev_log_index: 0,
            prev_log_term: 0,
            entries: vec![],
            leader_commit: 0,
        };

        let resp = follower.handle_append_entries(req);

        assert!(resp.success);
        assert_eq!(follower.leader_id(), Some(1));
    }

    #[test]
    fn test_append_entries_with_log() {
        let config = create_test_config();
        let mut follower = RaftNode::new(2, 1, 1, config.clone(), RaftConfig::default());

        let entries = vec![
            LogEntry {
                term: 1,
                index: 1,
                command: Command::Noop,
            },
            LogEntry {
                term: 1,
                index: 2,
                command: Command::Noop,
            },
        ];

        let req = AppendEntriesRequest {
            term: 1,
            leader_id: 1,
            prev_log_index: 0,
            prev_log_term: 0,
            entries,
            leader_commit: 1,
        };

        let resp = follower.handle_append_entries(req);

        assert!(resp.success);
        assert_eq!(resp.match_index, 2);
        assert_eq!(follower.commit_index(), 1);
    }

    #[test]
    fn test_config_change_add_node() {
        let config = create_test_config();
        let mut node = RaftNode::new(1, 1, 1, config, RaftConfig::default());

        // Become leader first
        node.become_candidate();
        node.handle_request_vote_response(
            2,
            RequestVoteResponse {
                term: 1,
                vote_granted: true,
            },
        );
        assert_eq!(node.state(), RaftState::Leader);

        // Propose adding a new node
        let result = node.propose_config_change(ConfigChange::AddNode {
            node_id: 4,
            addr: "localhost:9782".to_string(),
        });
        assert!(result.is_ok());

        // Apply the config change
        node.apply_config_change(&ConfigChange::AddNode {
            node_id: 4,
            addr: "localhost:9782".to_string(),
        });

        // Verify the node was added
        let cluster = node.cluster_config();
        assert!(cluster.get_node(4).is_some());
        assert_eq!(cluster.voters.len(), 4);
    }

    #[test]
    fn test_config_change_remove_node() {
        let config = create_test_config();
        let mut node = RaftNode::new(1, 1, 1, config, RaftConfig::default());

        // Become leader first
        node.become_candidate();
        node.handle_request_vote_response(
            2,
            RequestVoteResponse {
                term: 1,
                vote_granted: true,
            },
        );
        assert_eq!(node.state(), RaftState::Leader);

        // Apply removing node 3
        node.apply_config_change(&ConfigChange::RemoveNode { node_id: 3 });

        // Verify the node was removed
        let cluster = node.cluster_config();
        assert!(cluster.get_node(3).is_none());
        assert_eq!(cluster.voters.len(), 2);
    }

    #[test]
    fn test_install_snapshot() {
        let config = create_test_config();
        let mut follower = RaftNode::new(2, 1, 1, config, RaftConfig::default());

        let req = super::super::types::InstallSnapshotRequest {
            term: 2,
            leader_id: 1,
            last_included_index: 10,
            last_included_term: 2,
            offset: 0,
            data: vec![1, 2, 3, 4],
            done: true,
        };

        let (resp, action) = follower.handle_install_snapshot(req);

        assert_eq!(resp.term, 2);
        assert!(action.is_some());

        let action = action.unwrap();
        assert_eq!(action.last_included_index, 10);
        assert_eq!(action.last_included_term, 2);
        assert!(action.done);
    }

    #[test]
    fn test_apply_snapshot() {
        let config = create_test_config();
        let mut follower = RaftNode::new(2, 1, 1, config, RaftConfig::default());

        // Add some log entries first
        let entries = vec![
            LogEntry {
                term: 1,
                index: 1,
                command: Command::Noop,
            },
            LogEntry {
                term: 1,
                index: 2,
                command: Command::Noop,
            },
            LogEntry {
                term: 2,
                index: 3,
                command: Command::Noop,
            },
        ];
        let req = AppendEntriesRequest {
            term: 2,
            leader_id: 1,
            prev_log_index: 0,
            prev_log_term: 0,
            entries,
            leader_commit: 2,
        };
        follower.handle_append_entries(req);

        // Apply snapshot that covers all entries
        follower.apply_snapshot(10, 2);

        assert_eq!(follower.commit_index(), 10);
    }
}

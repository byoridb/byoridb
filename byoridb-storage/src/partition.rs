// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Partition management for Storage service
//!
//! This module provides:
//! - Partition ownership tracking
//! - Partition leadership management
//! - Request routing based on partition

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Partition status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionStatus {
    /// This node is the leader for this partition
    Leader,
    /// This node is a follower for this partition
    Follower,
    /// This node doesn't own this partition
    NotOwned,
    /// Partition is being transferred
    Transferring,
}

/// Partition information
#[derive(Debug, Clone)]
pub struct PartitionInfo {
    pub space_id: u32,
    pub part_id: u32,
    pub status: PartitionStatus,
    /// Leader host (host, port)
    pub leader: Option<(String, u32)>,
    /// All replicas including self
    pub peers: Vec<(String, u32)>,
}

/// Partition manager for a storage node
pub struct PartitionManager {
    /// This node's address
    local_addr: (String, u32),
    /// Owned partitions: (space_id, part_id) -> PartitionInfo
    partitions: Arc<RwLock<HashMap<(u32, u32), PartitionInfo>>>,
    /// Space to partition count mapping
    space_partition_nums: Arc<RwLock<HashMap<u32, u32>>>,
}

impl PartitionManager {
    /// Create a new partition manager
    pub fn new(host: String, port: u32) -> Self {
        Self {
            local_addr: (host, port),
            partitions: Arc::new(RwLock::new(HashMap::new())),
            space_partition_nums: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get local address
    pub fn local_addr(&self) -> &(String, u32) {
        &self.local_addr
    }

    /// Register a space with its partition count
    pub async fn register_space(&self, space_id: u32, partition_num: u32) {
        let mut space_parts = self.space_partition_nums.write().await;
        space_parts.insert(space_id, partition_num);
        debug!(
            "Registered space {} with {} partitions",
            space_id, partition_num
        );
    }

    /// Unregister a space
    pub async fn unregister_space(&self, space_id: u32) {
        // Remove partition count
        {
            let mut space_parts = self.space_partition_nums.write().await;
            space_parts.remove(&space_id);
        }

        // Remove all partitions for this space
        {
            let mut partitions = self.partitions.write().await;
            partitions.retain(|&(sid, _), _| sid != space_id);
        }

        info!("Unregistered space {}", space_id);
    }

    /// Add a partition that this node owns
    pub async fn add_partition(&self, info: PartitionInfo) {
        let key = (info.space_id, info.part_id);
        let status = info.status;

        let mut partitions = self.partitions.write().await;
        partitions.insert(key, info);

        debug!(
            "Added partition ({}, {}) with status {:?}",
            key.0, key.1, status
        );
    }

    /// Remove a partition
    pub async fn remove_partition(&self, space_id: u32, part_id: u32) {
        let mut partitions = self.partitions.write().await;
        partitions.remove(&(space_id, part_id));
        debug!("Removed partition ({}, {})", space_id, part_id);
    }

    /// Check if this node owns a partition
    pub async fn owns_partition(&self, space_id: u32, part_id: u32) -> bool {
        let partitions = self.partitions.read().await;
        partitions.contains_key(&(space_id, part_id))
    }

    /// Check if this node is the leader for a partition
    pub async fn is_leader(&self, space_id: u32, part_id: u32) -> bool {
        let partitions = self.partitions.read().await;
        partitions
            .get(&(space_id, part_id))
            .map(|p| p.status == PartitionStatus::Leader)
            .unwrap_or(false)
    }

    /// Get partition status
    pub async fn get_status(&self, space_id: u32, part_id: u32) -> PartitionStatus {
        let partitions = self.partitions.read().await;
        partitions
            .get(&(space_id, part_id))
            .map(|p| p.status)
            .unwrap_or(PartitionStatus::NotOwned)
    }

    /// Get partition info
    pub async fn get_partition(&self, space_id: u32, part_id: u32) -> Option<PartitionInfo> {
        let partitions = self.partitions.read().await;
        partitions.get(&(space_id, part_id)).cloned()
    }

    /// Get all partitions for a space
    pub async fn get_space_partitions(&self, space_id: u32) -> Vec<PartitionInfo> {
        let partitions = self.partitions.read().await;
        partitions
            .iter()
            .filter(|&(&(sid, _), _)| sid == space_id)
            .map(|(_, info)| info.clone())
            .collect()
    }

    /// Get all leader partitions for a space
    pub async fn get_leader_partitions(&self, space_id: u32) -> Vec<u32> {
        let partitions = self.partitions.read().await;
        partitions
            .iter()
            .filter(|&(&(sid, _), info)| sid == space_id && info.status == PartitionStatus::Leader)
            .map(|(&(_, part_id), _)| part_id)
            .collect()
    }

    /// Update partition status
    pub async fn update_status(&self, space_id: u32, part_id: u32, status: PartitionStatus) {
        let mut partitions = self.partitions.write().await;
        if let Some(info) = partitions.get_mut(&(space_id, part_id)) {
            info.status = status;
            debug!(
                "Updated partition ({}, {}) status to {:?}",
                space_id, part_id, status
            );
        }
    }

    /// Update partition leader
    pub async fn update_leader(&self, space_id: u32, part_id: u32, leader: Option<(String, u32)>) {
        let mut partitions = self.partitions.write().await;
        if let Some(info) = partitions.get_mut(&(space_id, part_id)) {
            if let Some(ref l) = leader {
                let is_self = *l == self.local_addr;
                info.leader = leader;
                info.status = if is_self {
                    PartitionStatus::Leader
                } else {
                    PartitionStatus::Follower
                };
            }
        }
    }

    /// Add a partition with specific status (convenience method)
    pub async fn add_partition_with_status(
        &self,
        space_id: u32,
        part_id: u32,
        status: PartitionStatus,
    ) {
        self.add_partition(PartitionInfo {
            space_id,
            part_id,
            status,
            leader: None,
            peers: vec![],
        })
        .await;
    }

    /// Get partition number for a space
    pub async fn get_partition_num(&self, space_id: u32) -> Option<u32> {
        let space_parts = self.space_partition_nums.read().await;
        space_parts.get(&space_id).copied()
    }

    /// Compute partition ID for a VID
    ///
    /// Note: This delegates to the centralized hash function in byoridb_common::hash
    #[inline]
    pub fn compute_part_id(vid: i64, partition_num: u32) -> u32 {
        byoridb_common::hash::compute_partition(vid, partition_num)
    }

    /// Validate that a request can be served by this node
    pub async fn validate_request(
        &self,
        space_id: u32,
        part_id: u32,
        require_leader: bool,
    ) -> Result<(), PartitionError> {
        let partitions = self.partitions.read().await;

        match partitions.get(&(space_id, part_id)) {
            None => Err(PartitionError::NotOwned { space_id, part_id }),
            Some(info) => {
                if require_leader && info.status != PartitionStatus::Leader {
                    Err(PartitionError::NotLeader {
                        space_id,
                        part_id,
                        leader: info.leader.clone(),
                    })
                } else {
                    Ok(())
                }
            }
        }
    }

    /// Initialize partitions for single-node mode (own all partitions as leader)
    pub async fn init_single_node(&self, space_id: u32, partition_num: u32) {
        self.register_space(space_id, partition_num).await;

        for part_id in 1..=partition_num {
            self.add_partition(PartitionInfo {
                space_id,
                part_id,
                status: PartitionStatus::Leader,
                leader: Some(self.local_addr.clone()),
                peers: vec![self.local_addr.clone()],
            })
            .await;
        }

        info!(
            "Initialized single-node mode for space {} with {} partitions",
            space_id, partition_num
        );
    }
}

/// Partition-related errors
#[derive(Debug, thiserror::Error)]
pub enum PartitionError {
    #[error("Partition ({space_id}, {part_id}) not owned by this node")]
    NotOwned { space_id: u32, part_id: u32 },

    #[error("Not leader for partition ({space_id}, {part_id}), leader is {leader:?}")]
    NotLeader {
        space_id: u32,
        part_id: u32,
        leader: Option<(String, u32)>,
    },

    #[error("Partition ({space_id}, {part_id}) is transferring")]
    Transferring { space_id: u32, part_id: u32 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_partition_manager() {
        let manager = PartitionManager::new("localhost".to_string(), 9779);

        // Register space
        manager.register_space(1, 10).await;

        // Add partition
        manager
            .add_partition(PartitionInfo {
                space_id: 1,
                part_id: 1,
                status: PartitionStatus::Leader,
                leader: Some(("localhost".to_string(), 9779)),
                peers: vec![("localhost".to_string(), 9779)],
            })
            .await;

        assert!(manager.owns_partition(1, 1).await);
        assert!(manager.is_leader(1, 1).await);
        assert!(!manager.owns_partition(1, 2).await);
    }

    #[tokio::test]
    async fn test_single_node_init() {
        let manager = PartitionManager::new("localhost".to_string(), 9779);

        manager.init_single_node(1, 5).await;

        for part_id in 1..=5 {
            assert!(manager.owns_partition(1, part_id).await);
            assert!(manager.is_leader(1, part_id).await);
        }

        assert!(!manager.owns_partition(1, 6).await);
    }

    #[test]
    fn test_compute_part_id() {
        // Same VID should always map to same partition
        let p1 = PartitionManager::compute_part_id(100, 10);
        let p2 = PartitionManager::compute_part_id(100, 10);
        assert_eq!(p1, p2);

        // Partition should be in range [1, partition_num]
        for vid in 0..1000 {
            let part = PartitionManager::compute_part_id(vid, 10);
            assert!(part >= 1 && part <= 10);
        }
    }
}

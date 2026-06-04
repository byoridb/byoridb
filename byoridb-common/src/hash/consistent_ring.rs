// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Consistent Hash Ring for partition assignment
//!
//! This module implements a consistent hash ring with virtual nodes to provide:
//! - Minimal data movement when nodes are added/removed (~1/N instead of ~90%)
//! - Uniform distribution of partitions across nodes
//! - Support for replication factor
//!
//! # Algorithm
//!
//! Each physical node is mapped to multiple positions on a hash ring using virtual nodes.
//! When looking up which node owns a key, we:
//! 1. Hash the key to get a position on the ring
//! 2. Walk clockwise to find the first node
//! 3. For replicas, continue walking to find N distinct physical nodes
//!
//! # Example
//!
//! ```
//! use byoridb_common::hash::{ConsistentHashRing, RingConfig, RingNode};
//!
//! let mut ring = ConsistentHashRing::new(10, 2, RingConfig::default());
//!
//! // Add storage nodes
//! ring.add_node(RingNode::new("host1".to_string(), 9779));
//! ring.add_node(RingNode::new("host2".to_string(), 9779));
//! ring.add_node(RingNode::new("host3".to_string(), 9779));
//!
//! // Get partition assignments
//! let assignments = ring.get_all_assignments();
//! assert_eq!(assignments.len(), 10);
//! ```

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Configuration for the consistent hash ring
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RingConfig {
    /// Number of virtual nodes per physical node
    /// Higher values = better distribution but more memory
    /// Recommended: 100-150
    pub virtual_nodes: u32,
}

impl Default for RingConfig {
    fn default() -> Self {
        Self { virtual_nodes: 150 }
    }
}

/// A physical node in the hash ring (storage host)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RingNode {
    pub host: String,
    pub port: u32,
}

impl RingNode {
    pub fn new(host: String, port: u32) -> Self {
        Self { host, port }
    }

    /// Unique identifier for this node
    pub fn id(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

impl From<(String, u32)> for RingNode {
    fn from((host, port): (String, u32)) -> Self {
        Self { host, port }
    }
}

impl From<&(String, u32)> for RingNode {
    fn from((host, port): &(String, u32)) -> Self {
        Self {
            host: host.clone(),
            port: *port,
        }
    }
}

/// Represents a partition assignment on the ring
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartitionAssignment {
    pub part_id: u32,
    /// Primary node (leader)
    pub primary: RingNode,
    /// Replica nodes (for replica_factor > 1)
    pub replicas: Vec<RingNode>,
}

impl PartitionAssignment {
    /// Get all nodes (primary + replicas) as host-port tuples
    pub fn all_hosts(&self) -> Vec<(String, u32)> {
        let mut hosts = vec![(self.primary.host.clone(), self.primary.port)];
        for replica in &self.replicas {
            hosts.push((replica.host.clone(), replica.port));
        }
        hosts
    }
}

/// A data migration task when ring topology changes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationTask {
    pub part_id: u32,
    pub from: RingNode,
    pub to: RingNode,
    pub is_primary: bool,
}

/// Consistent hash ring for partition assignment
///
/// Uses virtual nodes to improve distribution uniformity.
/// Each physical node maps to `virtual_nodes` positions on the ring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsistentHashRing {
    /// Ring configuration
    config: RingConfig,
    /// Sorted map: hash_position -> (node, virtual_node_index)
    ring: BTreeMap<u64, (RingNode, u32)>,
    /// Physical nodes in the ring
    nodes: Vec<RingNode>,
    /// Total number of partitions
    partition_num: u32,
    /// Replication factor
    replica_factor: u32,
    /// Cached partition assignments: part_id -> assignment
    /// Invalidated when ring changes
    #[serde(skip)]
    assignments_cache: Option<Vec<PartitionAssignment>>,
}

impl ConsistentHashRing {
    /// Create a new empty hash ring
    ///
    /// # Arguments
    /// * `partition_num` - Number of partitions in the space
    /// * `replica_factor` - Number of replicas per partition
    /// * `config` - Ring configuration (virtual nodes count)
    pub fn new(partition_num: u32, replica_factor: u32, config: RingConfig) -> Self {
        Self {
            config,
            ring: BTreeMap::new(),
            nodes: Vec::new(),
            partition_num,
            replica_factor,
            assignments_cache: None,
        }
    }

    /// Create a ring from existing host list
    pub fn from_hosts(
        partition_num: u32,
        replica_factor: u32,
        hosts: &[(String, u32)],
        config: RingConfig,
    ) -> Self {
        let mut ring = Self::new(partition_num, replica_factor, config);
        for (host, port) in hosts {
            ring.add_node(RingNode::new(host.clone(), *port));
        }
        ring
    }

    /// Add a node to the ring
    ///
    /// Creates virtual_nodes positions on the ring for this node.
    /// Invalidates the assignment cache.
    pub fn add_node(&mut self, node: RingNode) {
        if self.nodes.contains(&node) {
            return; // Already in ring
        }

        // Add virtual nodes
        for vn in 0..self.config.virtual_nodes {
            let key = format!("{}#{}", node.id(), vn);
            let hash = super::murmur::hash_bytes(key.as_bytes());
            self.ring.insert(hash, (node.clone(), vn));
        }

        self.nodes.push(node);
        self.assignments_cache = None; // Invalidate cache
    }

    /// Remove a node from the ring
    ///
    /// Removes all virtual node positions for this node.
    /// Invalidates the assignment cache.
    pub fn remove_node(&mut self, node: &RingNode) {
        if !self.nodes.contains(node) {
            return;
        }

        // Remove virtual nodes
        for vn in 0..self.config.virtual_nodes {
            let key = format!("{}#{}", node.id(), vn);
            let hash = super::murmur::hash_bytes(key.as_bytes());
            self.ring.remove(&hash);
        }

        self.nodes.retain(|n| n != node);
        self.assignments_cache = None; // Invalidate cache
    }

    /// Get the node responsible for a given hash value
    ///
    /// Walks clockwise from the hash position to find the first node.
    /// Returns None if ring is empty.
    pub fn get_node(&self, hash: u64) -> Option<&RingNode> {
        if self.ring.is_empty() {
            return None;
        }

        // Find the first node with hash >= input hash
        // If none found, wrap around to the first node
        self.ring
            .range(hash..)
            .next()
            .or_else(|| self.ring.iter().next())
            .map(|(_, (node, _))| node)
    }

    /// Get N distinct nodes for a given hash (for replication)
    ///
    /// Returns up to `count` nodes, or fewer if not enough distinct nodes exist.
    pub fn get_nodes(&self, hash: u64, count: u32) -> Vec<&RingNode> {
        if self.ring.is_empty() {
            return Vec::new();
        }

        let count = std::cmp::min(count as usize, self.nodes.len());
        let mut result = Vec::with_capacity(count);
        let mut seen_nodes = HashSet::new();

        // Start from the position and walk clockwise
        let iter = self.ring.range(hash..).chain(self.ring.iter());

        for (_, (node, _)) in iter {
            if seen_nodes.insert(node.id()) {
                result.push(node);
                if result.len() >= count {
                    break;
                }
            }
        }

        result
    }

    /// Get partition ID for a VID
    ///
    /// This maintains backward compatibility with existing partition_num concept.
    /// Uses the standard formula: (hash(vid) % partition_num) + 1
    #[inline]
    pub fn get_part_id(&self, vid: i64) -> u32 {
        super::murmur::compute_partition(vid, self.partition_num)
    }

    /// Get partition assignment (which nodes host a partition)
    pub fn get_partition_assignment(&mut self, part_id: u32) -> Option<PartitionAssignment> {
        self.ensure_assignments();
        self.assignments_cache
            .as_ref()
            .and_then(|a| a.get(part_id as usize - 1))
            .cloned()
    }

    /// Get all partition assignments
    pub fn get_all_assignments(&mut self) -> Vec<PartitionAssignment> {
        self.ensure_assignments();
        self.assignments_cache.clone().unwrap_or_default()
    }

    /// Convert assignments to HashMap format for compatibility
    pub fn get_allocations(&mut self) -> HashMap<u32, Vec<(String, u32)>> {
        let assignments = self.get_all_assignments();
        assignments
            .into_iter()
            .map(|a| (a.part_id, a.all_hosts()))
            .collect()
    }

    /// Compute all partition assignments (called when cache is invalid)
    fn ensure_assignments(&mut self) {
        if self.assignments_cache.is_some() {
            return;
        }

        let mut assignments = Vec::with_capacity(self.partition_num as usize);

        if self.nodes.is_empty() {
            // Fallback: no nodes registered, use localhost.
            // This keeps embedded/test setups functional but is unsafe in
            // production, so we emit a warning so operators notice a cluster
            // that has no registered storage nodes.
            Self::warn_localhost_fallback(self.partition_num, self.replica_factor);
            for part_id in 1..=self.partition_num {
                assignments.push(PartitionAssignment {
                    part_id,
                    primary: RingNode::new("localhost".to_string(), 9779),
                    replicas: Vec::new(),
                });
            }
        } else {
            for part_id in 1..=self.partition_num {
                // Hash the partition ID to get its position on the ring
                let hash = super::murmur::hash_bytes(&part_id.to_le_bytes());
                let nodes = self.get_nodes(hash, self.replica_factor);

                if let Some(primary) = nodes.first() {
                    assignments.push(PartitionAssignment {
                        part_id,
                        primary: (*primary).clone(),
                        replicas: nodes.iter().skip(1).map(|n| (*n).clone()).collect(),
                    });
                }
            }
        }

        self.assignments_cache = Some(assignments);
    }

    /// Emit a structured warning when the ring has no registered nodes and
    /// falls back to `localhost:9779`. Extracted so `ensure_assignments` stays
    /// below the project cognitive-complexity lint threshold.
    fn warn_localhost_fallback(partition_num: u32, replica_factor: u32) {
        tracing::warn!(
            partition_num,
            replica_factor,
            "ConsistentHashRing has no registered nodes; falling back to localhost:9779 for all {} partitions",
            partition_num
        );
    }

    /// Compute migration tasks when topology changes
    ///
    /// Returns a list of migrations needed to move data between nodes.
    pub fn compute_migrations(
        old_ring: &mut ConsistentHashRing,
        new_ring: &mut ConsistentHashRing,
    ) -> Vec<MigrationTask> {
        let mut tasks = Vec::new();

        let old_assignments = old_ring.get_all_assignments();
        let new_assignments = new_ring.get_all_assignments();

        for (old, new) in old_assignments.iter().zip(new_assignments.iter()) {
            // Check primary changes
            if old.primary != new.primary {
                tasks.push(MigrationTask {
                    part_id: old.part_id,
                    from: old.primary.clone(),
                    to: new.primary.clone(),
                    is_primary: true,
                });
            }

            // Check for replicas that need to move
            let old_replicas: HashSet<_> = old.replicas.iter().collect();
            let new_replicas: HashSet<_> = new.replicas.iter().collect();

            // Replicas removed (need to clean up on old nodes)
            for removed in old_replicas.difference(&new_replicas) {
                // Find where this replica data should go
                if let Some(new_node) = new_replicas.iter().find(|n| !old_replicas.contains(*n)) {
                    tasks.push(MigrationTask {
                        part_id: old.part_id,
                        from: (*removed).clone(),
                        to: (*new_node).clone(),
                        is_primary: false,
                    });
                }
            }
        }

        tasks
    }

    /// Number of nodes in the ring
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Check if ring is empty
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Get all physical nodes
    pub fn nodes(&self) -> &[RingNode] {
        &self.nodes
    }

    /// Get partition count
    pub fn partition_num(&self) -> u32 {
        self.partition_num
    }

    /// Get replica factor
    pub fn replica_factor(&self) -> u32 {
        self.replica_factor
    }

    /// Get ring configuration
    pub fn config(&self) -> &RingConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ring_new() {
        let ring = ConsistentHashRing::new(10, 2, RingConfig::default());
        assert_eq!(ring.partition_num(), 10);
        assert_eq!(ring.replica_factor(), 2);
        assert!(ring.is_empty());
    }

    #[test]
    fn test_ring_add_remove_node() {
        let mut ring = ConsistentHashRing::new(10, 3, RingConfig::default());
        let node1 = RingNode::new("host1".to_string(), 9779);
        let node2 = RingNode::new("host2".to_string(), 9779);

        ring.add_node(node1.clone());
        assert_eq!(ring.node_count(), 1);

        ring.add_node(node2.clone());
        assert_eq!(ring.node_count(), 2);

        // Adding duplicate should not increase count
        ring.add_node(node1.clone());
        assert_eq!(ring.node_count(), 2);

        ring.remove_node(&node1);
        assert_eq!(ring.node_count(), 1);

        // Removing non-existent should be no-op
        ring.remove_node(&node1);
        assert_eq!(ring.node_count(), 1);
    }

    #[test]
    fn test_ring_partition_assignment() {
        let mut ring = ConsistentHashRing::new(10, 2, RingConfig::default());
        for i in 1..=3 {
            ring.add_node(RingNode::new(format!("host{}", i), 9779));
        }

        // All partitions should have assignments
        let assignments = ring.get_all_assignments();
        assert_eq!(assignments.len(), 10);

        // Each assignment should have primary + 1 replica
        for a in &assignments {
            assert!(a.part_id >= 1 && a.part_id <= 10);
            assert_eq!(a.replicas.len(), 1);
            // Primary and replica should be different
            assert_ne!(a.primary, a.replicas[0]);
        }
    }

    #[test]
    fn test_ring_empty_fallback() {
        let mut ring = ConsistentHashRing::new(5, 2, RingConfig::default());

        // No nodes added - should fallback to localhost
        let assignments = ring.get_all_assignments();
        assert_eq!(assignments.len(), 5);

        for a in &assignments {
            assert_eq!(a.primary.host, "localhost");
            assert_eq!(a.primary.port, 9779);
            assert!(a.replicas.is_empty()); // No replicas when using fallback
        }
    }

    #[test]
    fn test_ring_stability() {
        let mut ring = ConsistentHashRing::new(100, 3, RingConfig::default());
        for i in 1..=5 {
            ring.add_node(RingNode::new(format!("host{}", i), 9779));
        }

        let assignments_before = ring.get_all_assignments();

        // Add one more node
        ring.add_node(RingNode::new("host6".to_string(), 9779));
        let assignments_after = ring.get_all_assignments();

        // Count how many partitions changed primary
        let changed: usize = assignments_before
            .iter()
            .zip(assignments_after.iter())
            .filter(|(b, a)| b.primary != a.primary)
            .count();

        // With 6 nodes, ~1/6 (~17%) should change
        // Allow 5-35% range for statistical variation
        assert!(
            changed >= 5 && changed <= 35,
            "Changed: {} (expected ~17%)",
            changed
        );
    }

    #[test]
    fn test_ring_from_hosts() {
        let hosts = vec![
            ("host1".to_string(), 9779),
            ("host2".to_string(), 9779),
            ("host3".to_string(), 9779),
        ];

        let mut ring = ConsistentHashRing::from_hosts(10, 2, &hosts, RingConfig::default());
        assert_eq!(ring.node_count(), 3);

        let assignments = ring.get_all_assignments();
        assert_eq!(assignments.len(), 10);
    }

    #[test]
    fn test_ring_get_allocations() {
        let hosts = vec![("host1".to_string(), 9779), ("host2".to_string(), 9779)];

        let mut ring = ConsistentHashRing::from_hosts(5, 2, &hosts, RingConfig::default());
        let allocations = ring.get_allocations();

        assert_eq!(allocations.len(), 5);
        for part_id in 1..=5 {
            let hosts = allocations.get(&part_id).unwrap();
            assert_eq!(hosts.len(), 2); // Primary + 1 replica
        }
    }

    #[test]
    fn test_ring_get_part_id() {
        let ring = ConsistentHashRing::new(10, 2, RingConfig::default());

        // Same VID always gets same partition
        let part1 = ring.get_part_id(12345);
        let part2 = ring.get_part_id(12345);
        assert_eq!(part1, part2);

        // Partition ID is in valid range
        for vid in 0..1000 {
            let part = ring.get_part_id(vid);
            assert!(part >= 1 && part <= 10);
        }
    }

    #[test]
    fn test_ring_serialization() {
        let mut ring = ConsistentHashRing::new(10, 2, RingConfig::default());
        ring.add_node(RingNode::new("host1".to_string(), 9779));
        ring.add_node(RingNode::new("host2".to_string(), 9779));

        let json = serde_json::to_string(&ring).unwrap();
        let restored: ConsistentHashRing = serde_json::from_str(&json).unwrap();

        assert_eq!(ring.node_count(), restored.node_count());
        assert_eq!(ring.partition_num(), restored.partition_num());
        assert_eq!(ring.replica_factor(), restored.replica_factor());
    }

    #[test]
    fn test_compute_migrations() {
        let mut old_ring = ConsistentHashRing::new(10, 2, RingConfig::default());
        old_ring.add_node(RingNode::new("host1".to_string(), 9779));
        old_ring.add_node(RingNode::new("host2".to_string(), 9779));
        old_ring.add_node(RingNode::new("host3".to_string(), 9779));

        let mut new_ring = old_ring.clone();
        new_ring.add_node(RingNode::new("host4".to_string(), 9779));

        let migrations = ConsistentHashRing::compute_migrations(&mut old_ring, &mut new_ring);

        // Some migrations should be needed
        // With 4 nodes, ~25% of data should move to the new node
        assert!(!migrations.is_empty());
        assert!(migrations.len() <= 10); // At most all partitions

        // All migrations should go TO the new node (host4)
        for task in &migrations {
            if task.is_primary {
                assert_eq!(task.to.host, "host4");
            }
        }
    }

    #[test]
    fn test_ring_distribution() {
        let mut ring = ConsistentHashRing::new(100, 1, RingConfig::default());
        for i in 1..=4 {
            ring.add_node(RingNode::new(format!("host{}", i), 9779));
        }

        let assignments = ring.get_all_assignments();

        // Count partitions per host
        let mut counts: HashMap<String, u32> = HashMap::new();
        for a in &assignments {
            *counts.entry(a.primary.host.clone()).or_insert(0) += 1;
        }

        // With 4 hosts and 100 partitions, each should have ~25 (±30%)
        for (host, count) in &counts {
            assert!(
                *count >= 15 && *count <= 40,
                "Host {} has {} partitions (expected ~25)",
                host,
                count
            );
        }
    }
}

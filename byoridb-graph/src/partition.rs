// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Partition routing for ByoriDB
//!
//! This module provides partition routing logic:
//! - VID to partition ID mapping
//! - Partition host resolution
//! - Consistent hashing for data distribution

use byoridb_meta::hotspot::HotspotDetector;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Partition router for mapping VIDs to partitions
pub struct PartitionRouter {
    /// Space ID -> (partition_num, part_id -> hosts)
    space_partitions: Arc<RwLock<HashMap<u32, SpacePartitionInfo>>>,
    /// Hotspot detector for tracking partition access patterns
    hotspot_detector: Option<Arc<HotspotDetector>>,
}

/// Partition information for a space
#[derive(Debug, Clone)]
pub struct SpacePartitionInfo {
    pub space_id: u32,
    pub partition_num: u32,
    pub replica_factor: u32,
    /// part_id -> list of (host, port)
    pub part_hosts: HashMap<u32, Vec<(String, u32)>>,
    /// Partition strategy for computing partition IDs
    pub partition_strategy: byoridb_common::PartitionStrategy,
}

/// Partition routing result
#[derive(Debug, Clone)]
pub struct PartitionRoute {
    pub part_id: u32,
    pub leader: Option<(String, u32)>,
    pub replicas: Vec<(String, u32)>,
}

impl PartitionRouter {
    /// Create a new partition router
    pub fn new() -> Self {
        Self {
            space_partitions: Arc::new(RwLock::new(HashMap::new())),
            hotspot_detector: None,
        }
    }

    /// Create a new partition router with hotspot detection enabled
    pub fn with_hotspot_detection(detector: Arc<HotspotDetector>) -> Self {
        Self {
            space_partitions: Arc::new(RwLock::new(HashMap::new())),
            hotspot_detector: Some(detector),
        }
    }

    /// Enable hotspot detection
    pub fn enable_hotspot_detection(&mut self, detector: Arc<HotspotDetector>) {
        self.hotspot_detector = Some(detector);
    }

    /// Get the hotspot detector if enabled
    pub fn get_hotspot_detector(&self) -> Option<&Arc<HotspotDetector>> {
        self.hotspot_detector.as_ref()
    }

    /// Register a space's partition information
    pub async fn register_space(&self, info: SpacePartitionInfo) {
        let mut spaces = self.space_partitions.write().await;
        spaces.insert(info.space_id, info);
    }

    /// Unregister a space
    pub async fn unregister_space(&self, space_id: u32) {
        let mut spaces = self.space_partitions.write().await;
        spaces.remove(&space_id);
    }

    /// Get partition ID for a VID in a space using the space's partition strategy
    pub async fn get_part_id(&self, space_id: u32, vid: i64) -> Option<u32> {
        let spaces = self.space_partitions.read().await;
        spaces.get(&space_id).map(|info| {
            info.partition_strategy
                .compute_partition(vid, info.partition_num)
        })
    }

    /// Get partition ID for a VID given partition_num (static method)
    ///
    /// Note: This delegates to the centralized hash function in byoridb_common::hash
    #[inline]
    pub fn hash_to_partition(vid: i64, partition_num: u32) -> u32 {
        byoridb_common::hash::compute_partition(vid, partition_num)
    }

    /// Get routing information for a VID using the space's partition strategy
    pub async fn route(&self, space_id: u32, vid: i64) -> Option<PartitionRoute> {
        self.route_with_tracking(space_id, vid, false).await
    }

    /// Get routing information for a VID with write tracking
    pub async fn route_write(&self, space_id: u32, vid: i64) -> Option<PartitionRoute> {
        self.route_with_tracking(space_id, vid, true).await
    }

    /// Internal routing method with optional hotspot tracking
    async fn route_with_tracking(
        &self,
        space_id: u32,
        vid: i64,
        is_write: bool,
    ) -> Option<PartitionRoute> {
        let spaces = self.space_partitions.read().await;
        let info = spaces.get(&space_id)?;

        let part_id = info
            .partition_strategy
            .compute_partition(vid, info.partition_num);
        let hosts = info.part_hosts.get(&part_id)?.clone();

        // Record request for hotspot detection
        if let Some(detector) = &self.hotspot_detector {
            detector.record_request(space_id, part_id, is_write);
        }

        Some(PartitionRoute {
            part_id,
            leader: hosts.first().cloned(),
            replicas: hosts,
        })
    }

    /// Get all partition IDs for a space
    pub async fn get_all_parts(&self, space_id: u32) -> Option<Vec<u32>> {
        let spaces = self.space_partitions.read().await;
        spaces
            .get(&space_id)
            .map(|info| (1..=info.partition_num).collect())
    }

    /// Get hosts for a specific partition
    pub async fn get_part_hosts(&self, space_id: u32, part_id: u32) -> Option<Vec<(String, u32)>> {
        let spaces = self.space_partitions.read().await;
        spaces
            .get(&space_id)
            .and_then(|info| info.part_hosts.get(&part_id))
            .cloned()
    }

    /// Update partition hosts (for rebalancing or failover)
    pub async fn update_part_hosts(&self, space_id: u32, part_id: u32, hosts: Vec<(String, u32)>) {
        let mut spaces = self.space_partitions.write().await;
        if let Some(info) = spaces.get_mut(&space_id) {
            info.part_hosts.insert(part_id, hosts);
        }
    }
}

impl Default for PartitionRouter {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper function to compute partition for a VID
pub fn compute_partition(vid: i64, partition_num: u32) -> u32 {
    PartitionRouter::hash_to_partition(vid, partition_num)
}

/// Helper function to compute partitions for multiple VIDs
pub fn compute_partitions(vids: &[i64], partition_num: u32) -> HashMap<u32, Vec<i64>> {
    let mut result: HashMap<u32, Vec<i64>> = HashMap::new();

    for &vid in vids {
        let part_id = compute_partition(vid, partition_num);
        result.entry(part_id).or_default().push(vid);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_to_partition() {
        // Test that same VID always maps to same partition
        let part1 = PartitionRouter::hash_to_partition(100, 10);
        let part2 = PartitionRouter::hash_to_partition(100, 10);
        assert_eq!(part1, part2);

        // Test partition is within range [1, partition_num]
        for vid in 0..1000 {
            let part = PartitionRouter::hash_to_partition(vid, 10);
            assert!((1..=10).contains(&part));
        }
    }

    #[test]
    fn test_partition_distribution() {
        // Test that partitions are reasonably distributed
        let partition_num = 10;
        let mut counts = vec![0u32; partition_num as usize + 1];

        for vid in 0..10000 {
            let part = PartitionRouter::hash_to_partition(vid, partition_num);
            counts[part as usize] += 1;
        }

        // Each partition should have roughly 1000 VIDs (±30%)
        for (i, count) in counts
            .iter()
            .enumerate()
            .take(partition_num as usize + 1)
            .skip(1)
        {
            assert!(*count > 700, "Partition {} has too few items: {}", i, count);
            assert!(
                *count < 1300,
                "Partition {} has too many items: {}",
                i,
                count
            );
        }
    }

    #[test]
    fn test_compute_partitions() {
        let vids = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let result = compute_partitions(&vids, 3);

        // All VIDs should be accounted for
        let total: usize = result.values().map(|v| v.len()).sum();
        assert_eq!(total, 10);

        // Partitions should be in range [1, 3]
        for &part_id in result.keys() {
            assert!((1..=3).contains(&part_id));
        }
    }

    #[tokio::test]
    async fn test_partition_router() {
        let router = PartitionRouter::new();

        // Register a space with Hash strategy
        let mut part_hosts = HashMap::new();
        part_hosts.insert(1, vec![("host1".to_string(), 9779)]);
        part_hosts.insert(2, vec![("host2".to_string(), 9779)]);
        part_hosts.insert(3, vec![("host3".to_string(), 9779)]);

        router
            .register_space(SpacePartitionInfo {
                space_id: 1,
                partition_num: 3,
                replica_factor: 1,
                part_hosts,
                partition_strategy: byoridb_common::PartitionStrategy::Hash,
            })
            .await;

        // Test routing
        let route = router.route(1, 100).await.unwrap();
        assert!(route.part_id >= 1 && route.part_id <= 3);
        assert!(route.leader.is_some());

        // Test getting all parts
        let parts = router.get_all_parts(1).await.unwrap();
        assert_eq!(parts, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn test_partition_router_with_range_strategy() {
        let router = PartitionRouter::new();

        // Register a space with Range strategy
        let mut part_hosts = HashMap::new();
        part_hosts.insert(1, vec![("host1".to_string(), 9779)]);
        part_hosts.insert(2, vec![("host2".to_string(), 9779)]);
        part_hosts.insert(3, vec![("host3".to_string(), 9779)]);
        part_hosts.insert(4, vec![("host4".to_string(), 9779)]);

        router
            .register_space(SpacePartitionInfo {
                space_id: 2,
                partition_num: 4,
                replica_factor: 1,
                part_hosts,
                partition_strategy: byoridb_common::PartitionStrategy::Range {
                    boundaries: vec![100, 200, 300],
                },
            })
            .await;

        // Test routing - VID 50 should go to partition 1 (< 100)
        let route = router.route(2, 50).await.unwrap();
        assert_eq!(route.part_id, 1);

        // VID 150 should go to partition 2 (>= 100, < 200)
        let route = router.route(2, 150).await.unwrap();
        assert_eq!(route.part_id, 2);

        // VID 250 should go to partition 3 (>= 200, < 300)
        let route = router.route(2, 250).await.unwrap();
        assert_eq!(route.part_id, 3);

        // VID 350 should go to partition 4 (>= 300)
        let route = router.route(2, 350).await.unwrap();
        assert_eq!(route.part_id, 4);
    }

    #[tokio::test]
    async fn test_partition_router_with_modulo_strategy() {
        let router = PartitionRouter::new();

        // Register a space with Modulo strategy
        let mut part_hosts = HashMap::new();
        for i in 1..=10 {
            part_hosts.insert(i, vec![(format!("host{}", i), 9779)]);
        }

        router
            .register_space(SpacePartitionInfo {
                space_id: 3,
                partition_num: 10,
                replica_factor: 1,
                part_hosts,
                partition_strategy: byoridb_common::PartitionStrategy::Modulo,
            })
            .await;

        // Test routing - VID 15 should go to partition 6 (15 % 10 + 1 = 6)
        let route = router.route(3, 15).await.unwrap();
        assert_eq!(route.part_id, 6);

        // VID 0 should go to partition 1 (0 % 10 + 1 = 1)
        let route = router.route(3, 0).await.unwrap();
        assert_eq!(route.part_id, 1);
    }
}

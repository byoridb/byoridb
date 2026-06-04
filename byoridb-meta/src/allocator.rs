// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Partition allocation strategy
//!
//! Distributes partitions across available storage hosts using round-robin
//! with replica spreading to ensure fault tolerance.

use std::collections::HashMap;

use tracing::warn;

/// Partition allocation result
#[derive(Debug, Clone)]
pub struct AllocationResult {
    /// part_id -> list of (host, port) replicas
    pub allocations: HashMap<u32, Vec<(String, u32)>>,
}

/// Partition allocator using round-robin strategy
pub struct PartitionAllocator;

impl PartitionAllocator {
    /// Allocate partitions across hosts
    ///
    /// Strategy: Round-robin with offset for replicas
    /// - Each partition gets `replica_factor` hosts
    /// - Replicas are spread across different hosts when possible
    ///
    /// # Arguments
    /// * `partition_num` - Number of partitions to allocate
    /// * `replica_factor` - Number of replicas per partition
    /// * `available_hosts` - List of available storage hosts
    ///
    /// # Returns
    /// `AllocationResult` containing partition-to-hosts mapping
    pub fn allocate(
        partition_num: u32,
        replica_factor: u32,
        available_hosts: &[(String, u32)],
    ) -> AllocationResult {
        let mut allocations = HashMap::new();

        if available_hosts.is_empty() {
            // Fallback to localhost if no hosts registered.
            // This lets embedded/test setups proceed, but is unsafe in real
            // deployments, so we emit a warning to make the fallback visible.
            warn!(
                partition_num,
                replica_factor,
                "PartitionAllocator received no hosts; allocating all {} partitions to localhost:9779",
                partition_num
            );
            for part_id in 1..=partition_num {
                allocations.insert(part_id, vec![("localhost".to_string(), 9779)]);
            }
            return AllocationResult { allocations };
        }

        let host_count = available_hosts.len();
        let effective_replica = std::cmp::min(replica_factor as usize, host_count);

        for part_id in 1..=partition_num {
            let mut replicas = Vec::with_capacity(effective_replica);

            // Select hosts for this partition using round-robin with offset
            for replica_idx in 0..effective_replica {
                let host_idx = ((part_id as usize - 1) + replica_idx) % host_count;
                let (host, port) = &available_hosts[host_idx];
                replicas.push((host.clone(), *port));
            }

            allocations.insert(part_id, replicas);
        }

        AllocationResult { allocations }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allocate_no_hosts_fallback() {
        let hosts: Vec<(String, u32)> = vec![];
        let result = PartitionAllocator::allocate(3, 2, &hosts);

        // Fallback to localhost
        for part_id in 1..=3 {
            let replicas = result.allocations.get(&part_id).unwrap();
            assert_eq!(replicas.len(), 1);
            assert_eq!(replicas[0], ("localhost".to_string(), 9779));
        }
    }

    #[test]
    fn test_allocate_single_host() {
        let hosts = vec![("host1".to_string(), 9779)];
        let result = PartitionAllocator::allocate(3, 3, &hosts);

        // With single host, all partitions go to that host (replica capped at 1)
        for part_id in 1..=3 {
            let replicas = result.allocations.get(&part_id).unwrap();
            assert_eq!(replicas.len(), 1);
            assert_eq!(replicas[0], ("host1".to_string(), 9779));
        }
    }

    #[test]
    fn test_allocate_multiple_hosts_with_replicas() {
        let hosts = vec![
            ("host1".to_string(), 9779),
            ("host2".to_string(), 9779),
            ("host3".to_string(), 9779),
        ];
        let result = PartitionAllocator::allocate(4, 2, &hosts);

        // Each partition should have 2 replicas on different hosts
        for part_id in 1..=4 {
            let replicas = result.allocations.get(&part_id).unwrap();
            assert_eq!(replicas.len(), 2);
            // Verify replicas are on different hosts
            assert_ne!(replicas[0].0, replicas[1].0);
        }

        // Verify round-robin distribution
        // Part 1: host1, host2
        // Part 2: host2, host3
        // Part 3: host3, host1
        // Part 4: host1, host2 (wraps around)
        assert_eq!(result.allocations.get(&1).unwrap()[0].0, "host1");
        assert_eq!(result.allocations.get(&1).unwrap()[1].0, "host2");
        assert_eq!(result.allocations.get(&2).unwrap()[0].0, "host2");
        assert_eq!(result.allocations.get(&2).unwrap()[1].0, "host3");
        assert_eq!(result.allocations.get(&3).unwrap()[0].0, "host3");
        assert_eq!(result.allocations.get(&3).unwrap()[1].0, "host1");
    }

    #[test]
    fn test_allocate_replica_factor_exceeds_hosts() {
        let hosts = vec![("host1".to_string(), 9779), ("host2".to_string(), 9779)];
        let result = PartitionAllocator::allocate(2, 5, &hosts);

        // Replica factor is capped at available hosts
        for part_id in 1..=2 {
            let replicas = result.allocations.get(&part_id).unwrap();
            assert_eq!(replicas.len(), 2); // Capped at 2 hosts
        }
    }

    #[test]
    fn test_allocate_full_replication() {
        let hosts = vec![
            ("host1".to_string(), 9779),
            ("host2".to_string(), 9779),
            ("host3".to_string(), 9779),
        ];
        let result = PartitionAllocator::allocate(2, 3, &hosts);

        // Each partition on all 3 hosts
        for part_id in 1..=2 {
            let replicas = result.allocations.get(&part_id).unwrap();
            assert_eq!(replicas.len(), 3);
        }
    }
}

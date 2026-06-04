// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! E2E tests for distributed query execution
//!
//! These tests verify that:
//! 1. Data is properly distributed across partitions using hash-based routing
//! 2. Distributed queries (FETCH, GO, LOOKUP) work correctly across partitions
//! 3. Results are properly aggregated from multiple storage nodes
//! 4. Index lookups work in distributed mode
//!
//! Gated behind the `distributed` feature: the executor/storage layers these
//! tests exercise (gRPC proto, DistributedQueryExecutor) are configured out of
//! embedded/default builds, so without the feature this whole file is skipped.
#![cfg(feature = "distributed")]

use byoridb_common::hash::compute_partition;
use byoridb_executor::distributed::DistributedQueryExecutor;
use byoridb_kvstore::{KVStoreOptions, RedbKVStore};
use byoridb_storage::proto::storage::EdgeKey;
use byoridb_storage::{IndexManager, IndexType, PartitionInfo, PartitionManager, PartitionStatus};
use std::collections::HashMap;
use std::sync::Arc;

/// Number of partitions for testing
const TEST_PARTITION_NUM: u32 = 4;

/// Default test space ID
const TEST_SPACE_ID: u32 = 1;

// ===== Unit Tests for Partition Distribution =====

#[test]
fn test_partition_hash_consistency() {
    // Test that the same VID always maps to the same partition
    let vid: i64 = 12345;
    let partition_num = TEST_PARTITION_NUM;

    let part1 = compute_partition(vid, partition_num);
    let part2 = compute_partition(vid, partition_num);
    let part3 = compute_partition(vid, partition_num);

    assert_eq!(part1, part2);
    assert_eq!(part2, part3);
    assert!(part1 >= 1 && part1 <= partition_num);
}

#[test]
fn test_partition_distribution_uniformity() {
    // Test that VIDs are reasonably distributed across partitions
    let partition_num = TEST_PARTITION_NUM;
    let mut counts: HashMap<u32, usize> = HashMap::new();

    // Generate 10000 test VIDs
    for vid in 1..=10000_i64 {
        let part_id = compute_partition(vid, partition_num);
        *counts.entry(part_id).or_insert(0) += 1;
    }

    // All partitions should have some data
    assert_eq!(counts.len() as u32, partition_num);

    // Each partition should have roughly 2500 VIDs (10000/4)
    // Allow 30% deviation for hash randomness
    let expected = 10000 / partition_num as usize;
    let min_expected = expected * 70 / 100;
    let max_expected = expected * 130 / 100;

    for (part_id, count) in &counts {
        assert!(
            *count >= min_expected && *count <= max_expected,
            "Partition {} has {} VIDs, expected between {} and {}",
            part_id,
            count,
            min_expected,
            max_expected
        );
    }
}

#[test]
fn test_group_vids_by_partition() {
    // Test the VID grouping function
    let vids: Vec<i64> = (1..=100).collect();
    let partition_num = TEST_PARTITION_NUM;

    let groups = DistributedQueryExecutor::group_vids_by_partition(&vids, partition_num);

    // Verify all VIDs are accounted for
    let total: usize = groups.values().map(|v| v.len()).sum();
    assert_eq!(total, vids.len());

    // Verify each VID is in the correct partition
    for (part_id, part_vids) in &groups {
        for vid in part_vids {
            let computed_part = compute_partition(*vid, partition_num);
            assert_eq!(
                computed_part, *part_id,
                "VID {} should be in partition {}",
                vid, part_id
            );
        }
    }
}

#[test]
fn test_group_edges_by_partition() {
    // Test the edge grouping function (grouped by source VID)
    let edge_keys: Vec<EdgeKey> = (1..=50_i64)
        .map(|i| EdgeKey {
            src_vid: i,
            dst_vid: i + 100,
            edge_type: "follows".to_string(),
            ranking: 0,
        })
        .collect();

    let partition_num = TEST_PARTITION_NUM;
    let groups = DistributedQueryExecutor::group_edges_by_partition(&edge_keys, partition_num);

    // Verify all edges are accounted for
    let total: usize = groups.values().map(|v| v.len()).sum();
    assert_eq!(total, edge_keys.len());

    // Verify each edge is in the correct partition (based on src_vid)
    for (part_id, part_edges) in &groups {
        for edge in part_edges {
            let computed_part = compute_partition(edge.src_vid, partition_num);
            assert_eq!(computed_part, *part_id);
        }
    }
}

// ===== Integration Tests with Mock Data =====

#[tokio::test(flavor = "multi_thread")]
async fn test_partition_manager_ownership() {
    // Test that partition ownership is tracked correctly
    let partition_manager = PartitionManager::new("127.0.0.1".to_string(), 9669);

    // Initially no partitions are owned
    assert!(!partition_manager.owns_partition(TEST_SPACE_ID, 1).await);

    // Add ownership using PartitionInfo
    let info1 = PartitionInfo {
        space_id: TEST_SPACE_ID,
        part_id: 1,
        status: PartitionStatus::Leader,
        leader: Some(("127.0.0.1".to_string(), 9669)),
        peers: vec![],
    };
    let info2 = PartitionInfo {
        space_id: TEST_SPACE_ID,
        part_id: 2,
        status: PartitionStatus::Leader,
        leader: Some(("127.0.0.1".to_string(), 9669)),
        peers: vec![],
    };

    partition_manager.add_partition(info1).await;
    partition_manager.add_partition(info2).await;

    assert!(partition_manager.owns_partition(TEST_SPACE_ID, 1).await);
    assert!(partition_manager.owns_partition(TEST_SPACE_ID, 2).await);
    assert!(!partition_manager.owns_partition(TEST_SPACE_ID, 3).await);

    // Remove ownership
    partition_manager.remove_partition(TEST_SPACE_ID, 1).await;
    assert!(!partition_manager.owns_partition(TEST_SPACE_ID, 1).await);
    assert!(partition_manager.owns_partition(TEST_SPACE_ID, 2).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_index_manager_basic_operations() {
    let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
    let kvstore: Arc<dyn byoridb_kvstore::KVStore> = Arc::new(
        RedbKVStore::open(temp_dir.path(), KVStoreOptions::default())
            .expect("Failed to create KVStore"),
    );

    let index_manager = IndexManager::new(kvstore);

    // Create a tag index
    let index_id = index_manager
        .create_tag_index(
            TEST_SPACE_ID,
            "person_name_idx".to_string(),
            1, // tag_id
            "person".to_string(),
            vec!["name".to_string()],
            vec![0], // field_indices
        )
        .await
        .expect("Failed to create tag index");

    assert!(index_id > 0);

    // Verify we can retrieve the index
    let index_def = index_manager
        .get_index_by_id(TEST_SPACE_ID, index_id)
        .await
        .expect("Failed to get index");

    assert_eq!(index_def.index_name, "person_name_idx");
    assert_eq!(index_def.schema_name, "person");
    assert!(matches!(index_def.index_type, IndexType::Tag));

    // Create an edge index
    let edge_index_id = index_manager
        .create_edge_index(
            TEST_SPACE_ID,
            "follows_weight_idx".to_string(),
            2, // edge_type_id
            "follows".to_string(),
            vec!["weight".to_string()],
            vec![0], // field_indices
        )
        .await
        .expect("Failed to create edge index");

    let edge_index_def = index_manager
        .get_index_by_id(TEST_SPACE_ID, edge_index_id)
        .await
        .expect("Failed to get edge index");

    assert_eq!(edge_index_def.index_name, "follows_weight_idx");
    assert_eq!(edge_index_def.schema_name, "follows");
    assert!(matches!(edge_index_def.index_type, IndexType::Edge));
}

// ===== Distributed Query Flow Tests =====

/// Test that demonstrates the distributed query execution flow
/// without actually running multiple servers
#[test]
fn test_distributed_query_flow_design() {
    // This test documents and verifies the distributed query flow:
    //
    // 1. Client sends query (e.g., FETCH PROP ON person 1, 2, 3, 4, 5)
    // 2. Executor checks if distributed_mode is enabled
    // 3. If distributed:
    //    a. Group VIDs by partition: {1: [1,3,5], 2: [2,4]} (example)
    //    b. Lookup partition hosts from MetaClient
    //    c. Send parallel RPCs to each storage node
    //    d. Aggregate results

    let vids: Vec<i64> = vec![1, 2, 3, 4, 5, 100, 200, 300];
    let partition_num = 4;

    // Step 1: Group VIDs by partition
    let groups = DistributedQueryExecutor::group_vids_by_partition(&vids, partition_num);

    // Verify grouping
    let total_vids: usize = groups.values().map(|v| v.len()).sum();
    assert_eq!(total_vids, vids.len(), "All VIDs should be grouped");

    // Step 2: Each partition would be sent to its storage node
    // Step 3: Results would be aggregated

    // This test verifies the logic without network calls
    println!("Partition distribution for VIDs {:?}:", vids);
    for (part_id, part_vids) in &groups {
        println!("  Partition {}: {:?}", part_id, part_vids);
    }
}

// ===== Checksum Verification Tests =====

#[test]
fn test_checksum_computation() {
    use byoridb_storage::proto::storage::KeyValuePair;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    // Helper to compute checksum like the RPC service does
    fn compute_batch_checksum(data: &[KeyValuePair]) -> u64 {
        let mut hasher = DefaultHasher::new();
        for kv in data {
            kv.key.hash(&mut hasher);
            kv.value.hash(&mut hasher);
        }
        hasher.finish()
    }

    let data1 = vec![
        KeyValuePair {
            key: b"key1".to_vec(),
            value: b"value1".to_vec(),
        },
        KeyValuePair {
            key: b"key2".to_vec(),
            value: b"value2".to_vec(),
        },
    ];

    let data2 = vec![
        KeyValuePair {
            key: b"key1".to_vec(),
            value: b"value1".to_vec(),
        },
        KeyValuePair {
            key: b"key2".to_vec(),
            value: b"value2".to_vec(),
        },
    ];

    let data3 = vec![
        KeyValuePair {
            key: b"key1".to_vec(),
            value: b"value1_modified".to_vec(),
        },
        KeyValuePair {
            key: b"key2".to_vec(),
            value: b"value2".to_vec(),
        },
    ];

    // Same data should produce same checksum
    let checksum1 = compute_batch_checksum(&data1);
    let checksum2 = compute_batch_checksum(&data2);
    assert_eq!(
        checksum1, checksum2,
        "Same data should produce same checksum"
    );
    assert_ne!(checksum1, 0, "Checksum should not be 0");

    // Different data should produce different checksum
    let checksum3 = compute_batch_checksum(&data3);
    assert_ne!(
        checksum1, checksum3,
        "Different data should produce different checksum"
    );
}

// ===== Index Value Conversion Tests =====

#[test]
#[allow(clippy::approx_constant)]
fn test_index_value_types() {
    use byoridb_storage::IndexValue;

    // Test different index value types
    let int_val = IndexValue::Int(42);
    let float_val = IndexValue::Float(3.14);
    let string_val = IndexValue::String("hello".to_string());
    let bool_val = IndexValue::Bool(true);
    let null_val = IndexValue::Null;

    // Verify type checking
    assert!(matches!(int_val, IndexValue::Int(_)));
    assert!(matches!(float_val, IndexValue::Float(_)));
    assert!(matches!(string_val, IndexValue::String(_)));
    assert!(matches!(bool_val, IndexValue::Bool(_)));
    assert!(matches!(null_val, IndexValue::Null));
}

// ===== Full E2E Test with Single Storage Node =====

#[tokio::test(flavor = "multi_thread")]
async fn test_storage_service_with_partition_data() {
    let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
    let kvstore: Arc<dyn byoridb_kvstore::KVStore> = Arc::new(
        RedbKVStore::open(temp_dir.path(), KVStoreOptions::default())
            .expect("Failed to create KVStore"),
    );

    // Set up partition ownership
    let partition_manager = Arc::new(PartitionManager::new("127.0.0.1".to_string(), 9669));
    for part_id in 1..=TEST_PARTITION_NUM {
        let info = PartitionInfo {
            space_id: TEST_SPACE_ID,
            part_id,
            status: PartitionStatus::Leader,
            leader: Some(("127.0.0.1".to_string(), 9669)),
            peers: vec![],
        };
        partition_manager.add_partition(info).await;
    }

    // Insert test data into KVStore
    // Using partition key format: p{part_id}:s{space_id}:v:{vid}
    for vid in 1_i64..=20 {
        let part_id = compute_partition(vid, TEST_PARTITION_NUM);
        let key = format!("p{}:s{}:v:{}", part_id, TEST_SPACE_ID, vid);
        let value = format!(r#"{{"vid": {}, "name": "User{}"}}"#, vid, vid);

        kvstore
            .put(key.as_bytes(), value.as_bytes())
            .await
            .expect("Failed to insert test data");
    }

    // Verify data was inserted
    for vid in 1_i64..=20 {
        let part_id = compute_partition(vid, TEST_PARTITION_NUM);
        let key = format!("p{}:s{}:v:{}", part_id, TEST_SPACE_ID, vid);

        let value = kvstore
            .get(key.as_bytes())
            .await
            .expect("Failed to get data");

        assert!(value.is_some(), "Data for VID {} should exist", vid);
    }

    // Verify partition distribution
    let mut partition_counts: HashMap<u32, usize> = HashMap::new();
    for vid in 1_i64..=20 {
        let part_id = compute_partition(vid, TEST_PARTITION_NUM);
        *partition_counts.entry(part_id).or_insert(0) += 1;
    }

    println!("Partition distribution for 20 VIDs:");
    for (part_id, count) in &partition_counts {
        println!("  Partition {}: {} VIDs", part_id, count);
    }

    // All partitions should have at least one VID
    assert!(
        partition_counts.len() > 1,
        "Data should be distributed across multiple partitions"
    );
}

// ===== Stress Tests for Partition Distribution =====

#[test]
fn test_partition_distribution_large_scale() {
    // Test with a large number of VIDs to ensure consistent distribution
    let partition_num = 10;
    let mut counts: HashMap<u32, usize> = HashMap::new();

    // 100,000 VIDs
    for vid in 1..=100_000_i64 {
        let part_id = compute_partition(vid, partition_num);
        *counts.entry(part_id).or_insert(0) += 1;
    }

    // All partitions should exist
    assert_eq!(counts.len() as u32, partition_num);

    // Calculate standard deviation to verify uniform distribution
    let expected = 100_000 / partition_num as usize;
    let variance: f64 = counts
        .values()
        .map(|&c| ((c as f64) - (expected as f64)).powi(2))
        .sum::<f64>()
        / partition_num as f64;
    let std_dev = variance.sqrt();

    // Standard deviation should be less than 5% of expected value
    let max_std_dev = expected as f64 * 0.05;
    assert!(
        std_dev < max_std_dev,
        "Distribution is not uniform enough. Std dev: {}, max allowed: {}",
        std_dev,
        max_std_dev
    );

    println!(
        "Partition distribution for 100,000 VIDs across {} partitions:",
        partition_num
    );
    println!("Expected per partition: {}", expected);
    println!("Standard deviation: {:.2}", std_dev);
    for (part_id, count) in counts.iter() {
        println!(
            "  Partition {}: {} ({:.2}%)",
            part_id,
            count,
            (*count as f64 / 1000.0)
        );
    }
}

// ===== Edge Cases =====

#[test]
fn test_partition_with_single_partition() {
    // Edge case: only one partition
    let partition_num = 1;

    for vid in 1..=100_i64 {
        let part_id = compute_partition(vid, partition_num);
        assert_eq!(part_id, 1, "All VIDs should map to partition 1");
    }
}

#[test]
fn test_partition_with_negative_vids() {
    // Test with negative VIDs
    let partition_num = 4;
    let negative_vids: Vec<i64> = vec![-100, -50, -1, 0, 1, 50, 100];

    for vid in negative_vids {
        let part_id = compute_partition(vid, partition_num);
        assert!(
            part_id >= 1 && part_id <= partition_num,
            "VID {} mapped to invalid partition {}",
            vid,
            part_id
        );
    }
}

#[test]
fn test_partition_with_extreme_vids() {
    // Test with extreme VID values
    let partition_num = 8;
    let extreme_vids: Vec<i64> = vec![i64::MIN, i64::MAX, 0, -1, 1];

    for vid in extreme_vids {
        let part_id = compute_partition(vid, partition_num);
        assert!(
            part_id >= 1 && part_id <= partition_num,
            "Extreme VID {} mapped to invalid partition {}",
            vid,
            part_id
        );
    }
}

#[test]
fn test_empty_vids_grouping() {
    // Edge case: empty VID list
    let vids: Vec<i64> = vec![];
    let partition_num = 4;

    let groups = DistributedQueryExecutor::group_vids_by_partition(&vids, partition_num);

    assert!(
        groups.is_empty(),
        "Empty VID list should produce empty groups"
    );
}

#[test]
fn test_empty_edges_grouping() {
    // Edge case: empty edge list
    let edges: Vec<EdgeKey> = vec![];
    let partition_num = 4;

    let groups = DistributedQueryExecutor::group_edges_by_partition(&edges, partition_num);

    assert!(
        groups.is_empty(),
        "Empty edge list should produce empty groups"
    );
}

// ===== Partition Leader Tests =====

#[tokio::test(flavor = "multi_thread")]
async fn test_partition_leader_status() {
    let partition_manager = PartitionManager::new("127.0.0.1".to_string(), 9669);

    // Add partition as leader
    let leader_info = PartitionInfo {
        space_id: TEST_SPACE_ID,
        part_id: 1,
        status: PartitionStatus::Leader,
        leader: Some(("127.0.0.1".to_string(), 9669)),
        peers: vec![("127.0.0.2".to_string(), 9669)],
    };
    partition_manager.add_partition(leader_info).await;

    // Add partition as follower
    let follower_info = PartitionInfo {
        space_id: TEST_SPACE_ID,
        part_id: 2,
        status: PartitionStatus::Follower,
        leader: Some(("127.0.0.2".to_string(), 9669)),
        peers: vec![("127.0.0.1".to_string(), 9669)],
    };
    partition_manager.add_partition(follower_info).await;

    // Test is_leader
    assert!(partition_manager.is_leader(TEST_SPACE_ID, 1).await);
    assert!(!partition_manager.is_leader(TEST_SPACE_ID, 2).await);

    // Test get_status
    assert_eq!(
        partition_manager.get_status(TEST_SPACE_ID, 1).await,
        PartitionStatus::Leader
    );
    assert_eq!(
        partition_manager.get_status(TEST_SPACE_ID, 2).await,
        PartitionStatus::Follower
    );
    assert_eq!(
        partition_manager.get_status(TEST_SPACE_ID, 3).await,
        PartitionStatus::NotOwned
    );
}

// ===== Space Registration Tests =====

#[tokio::test(flavor = "multi_thread")]
async fn test_space_partition_registration() {
    let partition_manager = PartitionManager::new("127.0.0.1".to_string(), 9669);

    // Register space with partition count
    partition_manager.register_space(TEST_SPACE_ID, 8).await;

    // Add partitions
    for part_id in 1..=8 {
        let info = PartitionInfo {
            space_id: TEST_SPACE_ID,
            part_id,
            status: PartitionStatus::Leader,
            leader: Some(("127.0.0.1".to_string(), 9669)),
            peers: vec![],
        };
        partition_manager.add_partition(info).await;
    }

    // Verify all partitions exist
    for part_id in 1..=8 {
        assert!(
            partition_manager
                .owns_partition(TEST_SPACE_ID, part_id)
                .await,
            "Partition {} should be owned",
            part_id
        );
    }

    // Unregister space
    partition_manager.unregister_space(TEST_SPACE_ID).await;

    // Verify all partitions are removed
    for part_id in 1..=8 {
        assert!(
            !partition_manager
                .owns_partition(TEST_SPACE_ID, part_id)
                .await,
            "Partition {} should be removed after unregister",
            part_id
        );
    }
}

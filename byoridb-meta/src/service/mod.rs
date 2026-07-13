// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Meta service implementation
//!
//! Uses DashMap for lock-free concurrent reads and AtomicU32 for ID generation.
//! This provides significant performance improvement over RwLock<HashMap> for
//! concurrent metadata access patterns.

use super::error::{MetaError, Result};
use super::key::MetaKey;
use super::schema::*;
use byoridb_common::hash::{ConsistentHashRing, RingConfig, RingNode};
use byoridb_kvstore::KVStore;
use dashmap::DashMap;
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// Host status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostStatus {
    Online,
    Offline,
}

/// Storage host information for tracking registered nodes
#[derive(Debug, Clone)]
pub struct StorageHostInfo {
    pub host: String,
    pub port: u32,
    pub last_heartbeat: Instant,
    pub status: HostStatus,
    /// Partitions this host owns: (space_id, part_id)
    pub partitions: HashSet<(u32, u32)>,
}

/// Heartbeat response info
#[derive(Debug, Clone)]
pub struct HeartbeatInfo {
    pub cluster_id: i64,
}

/// Summary row for a storage host, including its live counts.
///
/// Returned by [`MetaService::list_hosts_with_counts`] and exposed through
/// the `ListHosts` gRPC.
#[derive(Debug, Clone)]
pub struct HostSummary {
    pub host: String,
    pub port: u32,
    pub status: HostStatus,
    /// Number of partitions for which this host is the designated leader
    /// (the first host in the allocation's host list).
    pub leader_count: u64,
    /// Total number of (space, partition) pairs owned by this host.
    pub part_count: u64,
}

/// Meta service with lock-free concurrent data structures
///
/// DashMap provides:
/// - Lock-free reads (no contention for concurrent queries)
/// - Fine-grained locking for writes (per-shard, not global)
/// - Better scalability under concurrent workloads
pub struct MetaService {
    kvstore: Arc<dyn KVStore>,

    // Spaces: lock-free concurrent access
    spaces: DashMap<u32, Space>,
    space_names: DashMap<String, u32>,
    next_space_id: AtomicU32,

    /// Partition allocations: space_id -> (part_id -> hosts)
    part_allocations: DashMap<u32, HashMap<u32, Vec<(String, u32)>>>,

    /// Active storage hosts: (host, port) -> StorageHostInfo
    storage_hosts: DashMap<(String, u32), StorageHostInfo>,

    /// Host liveness timeout (default: 30 seconds)
    host_timeout: Duration,

    /// Consistent hash rings for partition assignment: space_id -> ring
    /// Uses RwLock because ConsistentHashRing requires mutable access for cached assignments
    hash_rings: DashMap<u32, RwLock<ConsistentHashRing>>,

    // Tags: lock-free concurrent access
    tags: DashMap<(u32, u32), TagSchema>,
    tag_names: DashMap<(u32, String), u32>,
    next_tag_id: AtomicU32,

    // Edges: lock-free concurrent access
    edges: DashMap<(u32, u32), EdgeSchema>,
    edge_names: DashMap<(u32, String), u32>,
    next_edge_id: AtomicU32,

    // Tag indexes: lock-free concurrent access
    tag_indexes: DashMap<(u32, u32), TagIndex>,
    tag_index_names: DashMap<(u32, String), u32>,
    next_tag_index_id: AtomicU32,

    // Edge indexes: lock-free concurrent access
    edge_indexes: DashMap<(u32, u32), EdgeIndex>,
    edge_index_names: DashMap<(u32, String), u32>,
    next_edge_index_id: AtomicU32,

    /// Cluster identity. Heartbeat requests must carry this value (or 0 for
    /// first-time registration). Prevents rogue nodes from joining.
    pub cluster_id: i64,
}

impl MetaService {
    pub fn new(kvstore: Arc<dyn KVStore>) -> Self {
        // Generate a random cluster_id at startup. All nodes joining this
        // cluster must present this value in heartbeat requests.
        let cluster_id = {
            use std::time::{SystemTime, UNIX_EPOCH};
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos() as i64;
            // Mix with a compile-time constant to reduce collision probability
            nanos ^ 0x6c62272e07bb0142
        };
        MetaService {
            kvstore,
            spaces: DashMap::new(),
            space_names: DashMap::new(),
            next_space_id: AtomicU32::new(1),
            part_allocations: DashMap::new(),
            storage_hosts: DashMap::new(),
            host_timeout: Duration::from_secs(30),
            hash_rings: DashMap::new(),
            tags: DashMap::new(),
            tag_names: DashMap::new(),
            next_tag_id: AtomicU32::new(1),
            edges: DashMap::new(),
            edge_names: DashMap::new(),
            next_edge_id: AtomicU32::new(1),
            tag_indexes: DashMap::new(),
            tag_index_names: DashMap::new(),
            next_tag_index_id: AtomicU32::new(1),
            edge_indexes: DashMap::new(),
            edge_index_names: DashMap::new(),
            next_edge_index_id: AtomicU32::new(1),
            cluster_id,
        }
    }
}

mod edge;
mod host;
mod index;
mod partition;
mod space;
mod tag;

#[cfg(test)]
mod tests {
    use super::*;
    use byoridb_kvstore::store::MemoryKVStore;

    #[tokio::test]
    async fn test_alter_tag_success() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let meta = MetaService::new(kvstore);

        // Create space
        let space_id = meta
            .create_space(
                "test_space".to_string(),
                10,
                1,
                VidType::Int64,
                byoridb_common::PartitionStrategy::Hash,
            )
            .await
            .unwrap();

        // Create tag
        meta.create_tag(
            space_id,
            "test_tag".to_string(),
            vec![Field {
                name: "prop1".to_string(),
                data_type: DataType::Int64,
                nullable: false,
                default: None,
            }],
        )
        .await
        .unwrap();

        // Alter tag: Add new column
        let operations = vec![AlterOperation::AddColumn(Field {
            name: "prop2".to_string(),
            data_type: DataType::String,
            nullable: true,
            default: None,
        })];

        let new_version = meta
            .alter_tag(space_id, "test_tag", operations)
            .await
            .unwrap();
        assert_eq!(new_version, 2);

        // Verify new field exists
        let tag = meta.get_tag(space_id, "test_tag").await.unwrap();
        assert_eq!(tag.fields.len(), 2);
        assert_eq!(tag.fields[1].name, "prop2");
        assert_eq!(tag.version, 2);
    }

    #[tokio::test]
    async fn test_alter_tag_validation() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let meta = MetaService::new(kvstore);

        let space_id = meta
            .create_space(
                "test_space_val".to_string(),
                10,
                1,
                VidType::Int64,
                byoridb_common::PartitionStrategy::Hash,
            )
            .await
            .unwrap();
        meta.create_tag(space_id, "test_tag_val".to_string(), vec![])
            .await
            .unwrap();

        // Try adding non-nullable column without default - should fail
        let operations = vec![AlterOperation::AddColumn(Field {
            name: "invalid_col".to_string(),
            data_type: DataType::Int64,
            nullable: false,
            default: None,
        })];

        let result = meta.alter_tag(space_id, "test_tag_val", operations).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            MetaError::InvalidAlterOperation(_) => (),
            e => panic!("Expected InvalidAlterOperation, got {:?}", e),
        }

        // Try adding non-nullable column WITH default - should succeed
        let operations_success = vec![AlterOperation::AddColumn(Field {
            name: "valid_col".to_string(),
            data_type: DataType::Int64,
            nullable: false,
            default: Some("10".to_string()),
        })];

        let new_version = meta
            .alter_tag(space_id, "test_tag_val", operations_success)
            .await
            .unwrap();
        assert_eq!(new_version, 2);
    }

    #[tokio::test]
    async fn test_alter_tag_duplicate_field() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let meta = MetaService::new(kvstore);

        let space_id = meta
            .create_space(
                "test_space_dup".to_string(),
                10,
                1,
                VidType::Int64,
                byoridb_common::PartitionStrategy::Hash,
            )
            .await
            .unwrap();
        meta.create_tag(
            space_id,
            "test_tag_dup".to_string(),
            vec![Field {
                name: "prop1".to_string(),
                data_type: DataType::Int64,
                nullable: false,
                default: None,
            }],
        )
        .await
        .unwrap();

        // Try adding existing column
        let operations = vec![AlterOperation::AddColumn(Field {
            name: "prop1".to_string(),
            data_type: DataType::Int64,
            nullable: true,
            default: None,
        })];

        let result = meta.alter_tag(space_id, "test_tag_dup", operations).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            MetaError::FieldAlreadyExists(_) => (),
            e => panic!("Expected FieldAlreadyExists, got {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_alter_edge_success() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let meta = MetaService::new(kvstore);

        let space_id = meta
            .create_space(
                "test_space_edge".to_string(),
                10,
                1,
                VidType::Int64,
                byoridb_common::PartitionStrategy::Hash,
            )
            .await
            .unwrap();
        meta.create_edge(
            space_id,
            "test_edge".to_string(),
            vec![Field {
                name: "src".to_string(),
                data_type: DataType::Int64,
                nullable: false,
                default: None,
            }],
        )
        .await
        .unwrap();

        // Alter edge: Add new column
        let operations = vec![AlterOperation::AddColumn(Field {
            name: "weight".to_string(),
            data_type: DataType::Double,
            nullable: false,
            default: Some("1.0".to_string()),
        })];

        let new_version = meta
            .alter_edge(space_id, "test_edge", operations)
            .await
            .unwrap();
        assert_eq!(new_version, 2);

        // Verify new field exists
        let edge = meta.get_edge(space_id, "test_edge").await.unwrap();
        assert_eq!(edge.fields.len(), 2);
        assert_eq!(edge.fields[1].name, "weight");
    }

    #[tokio::test]
    async fn test_tag_version_history() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let meta = MetaService::new(kvstore);

        let space_id = meta
            .create_space(
                "test_space_hist".to_string(),
                10,
                1,
                VidType::Int64,
                byoridb_common::PartitionStrategy::Hash,
            )
            .await
            .unwrap();
        meta.create_tag(space_id, "test_tag_hist".to_string(), vec![])
            .await
            .unwrap();

        // v1 -> v2
        meta.alter_tag(
            space_id,
            "test_tag_hist",
            vec![AlterOperation::AddColumn(Field {
                name: "col1".to_string(),
                data_type: DataType::Int64,
                nullable: true,
                default: None,
            })],
        )
        .await
        .unwrap();

        // v2 -> v3
        meta.alter_tag(
            space_id,
            "test_tag_hist",
            vec![AlterOperation::AddColumn(Field {
                name: "col2".to_string(),
                data_type: DataType::Int64,
                nullable: true,
                default: None,
            })],
        )
        .await
        .unwrap();

        // Get versions
        let versions = meta
            .get_tag_versions(space_id, "test_tag_hist")
            .await
            .unwrap();
        assert_eq!(versions.len(), 3);
        assert_eq!(versions[0].version, 1);
        assert_eq!(versions[1].version, 2);
        assert_eq!(versions[2].version, 3);
    }

    /// With no heartbeats received, the host list is empty (not a placeholder).
    #[tokio::test]
    async fn test_list_hosts_empty_when_no_heartbeats() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let meta = MetaService::new(kvstore);

        let hosts = meta.list_hosts_with_counts();
        assert!(
            hosts.is_empty(),
            "expected no hosts before any heartbeat, got {:?}",
            hosts
        );
    }

    /// A registered host with no partitions should report zero leader/part
    /// counts and `Online` status immediately after a heartbeat.
    #[tokio::test]
    async fn test_list_hosts_online_after_heartbeat() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let meta = MetaService::new(kvstore);

        meta.handle_heartbeat("10.0.0.1".to_string(), 9779, "storage", 0)
            .unwrap();

        let hosts = meta.list_hosts_with_counts();
        assert_eq!(hosts.len(), 1);
        let h = &hosts[0];
        assert_eq!(h.host, "10.0.0.1");
        assert_eq!(h.port, 9779);
        assert_eq!(h.status, HostStatus::Online);
        assert_eq!(h.leader_count, 0);
        assert_eq!(h.part_count, 0);
    }

    /// After creating a space, registered hosts should have their part counts
    /// updated and the first host in each allocation should be counted as
    /// leader for that partition.
    #[tokio::test]
    async fn test_list_hosts_reports_leader_and_part_counts() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let meta = MetaService::new(kvstore);

        // Register two storage hosts first so the allocator can place partitions
        // on them at space-creation time.
        meta.handle_heartbeat("10.0.0.1".to_string(), 9779, "storage", 0)
            .unwrap();
        meta.handle_heartbeat("10.0.0.2".to_string(), 9779, "storage", 0)
            .unwrap();

        meta.create_space(
            "parts_space".to_string(),
            4,
            1,
            VidType::Int64,
            byoridb_common::PartitionStrategy::Hash,
        )
        .await
        .unwrap();

        let hosts = meta.list_hosts_with_counts();
        assert_eq!(hosts.len(), 2);

        // Total leader-count across hosts must match the number of partitions,
        // because every partition has exactly one "first host".
        let total_leaders: u64 = hosts.iter().map(|h| h.leader_count).sum();
        assert_eq!(total_leaders, 4);

        // Total part-count across hosts equals partition_num * replica_factor.
        let total_parts: u64 = hosts.iter().map(|h| h.part_count).sum();
        assert_eq!(total_parts, 4);

        // All hosts are fresh so they must be Online.
        for h in &hosts {
            assert_eq!(h.status, HostStatus::Online, "host {:?}", h);
        }
    }

    /// Non-storage roles (graph, meta, unknown) must still succeed but must
    /// not leak into `storage_hosts`. This test locks in the documented
    /// behaviour and guards the observability path (debug-level log) against
    /// regressions that would put the handler back into a silent `Ok(..)`.
    #[tokio::test]
    async fn test_heartbeat_non_storage_role_is_not_registered() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let meta = MetaService::new(kvstore);

        meta.handle_heartbeat("graph-node".to_string(), 9669, "graph", 0)
            .unwrap();
        meta.handle_heartbeat("meta-node".to_string(), 9559, "meta", 0)
            .unwrap();
        meta.handle_heartbeat("weirdo".to_string(), 1234, "unknown-role", 0)
            .unwrap();

        assert_eq!(
            meta.storage_host_count(),
            0,
            "non-storage roles must not populate the storage host registry"
        );
        assert!(
            meta.list_hosts_with_counts().is_empty(),
            "SHOW HOSTS must remain empty when only non-storage nodes have pinged"
        );
    }

    /// Positive counterpart: a storage role still registers, confirming the
    /// role gate didn't accidentally drop the intended path.
    #[tokio::test]
    async fn test_heartbeat_storage_role_still_registers() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let meta = MetaService::new(kvstore);

        meta.handle_heartbeat("storage-node".to_string(), 9779, "storage", 0)
            .unwrap();

        assert_eq!(meta.storage_host_count(), 1);
        let hosts = meta.list_hosts_with_counts();
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].host, "storage-node");
        assert_eq!(hosts[0].port, 9779);
    }
}

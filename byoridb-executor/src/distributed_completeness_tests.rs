//! Real Meta + two Storage RPC endpoints, with no graph data in the coordinator.
use super::*;
use crate::context::{ExecutionConfig, ExecutionContext};
use crate::executor::{Executor, ExecutorResult};
use crate::plan::ExecutionPlanBuilder;
use byoridb_codec::{EdgeData as StoredEdge, TagData, VertexCodec, VertexData as StoredVertex};
use byoridb_common::Value;
use byoridb_kvstore::{KVStore, MemoryKVStore};
use byoridb_meta::proto::meta_service_server::MetaServiceServer;
use byoridb_meta::rpc::MetaRpcService;
use byoridb_storage::index::IndexManager;
use byoridb_storage::key::IndexValue as StoredIndexValue;
use byoridb_storage::partition::{PartitionManager, PartitionStatus};
use byoridb_storage::proto::storage::storage_service_server::StorageServiceServer;
use byoridb_storage::rpc::StorageRpcService;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

fn incoming(
    listener: TcpListener,
) -> impl futures::Stream<Item = std::io::Result<tokio::net::TcpStream>> {
    futures::stream::unfold(listener, |listener| async {
        let connection = listener.accept().await.map(|(connection, _)| connection);
        Some((connection, listener))
    })
}

struct Fixture {
    stores: Vec<Arc<MemoryKVStore>>,
    indexes: Vec<Arc<IndexManager>>,
    partitions: Vec<Arc<PartitionManager>>,
    ports: Vec<u32>,
    storage: Arc<StorageQueryClient>,
    meta: Arc<MetaClient>,
    coordinator: Arc<MemoryKVStore>,
    coordinator_indexes: Arc<IndexManager>,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

async fn create_indexes(manager: &IndexManager) {
    assert_eq!(
        manager
            .create_tag_index(
                1,
                "tag_score".into(),
                1,
                "item".into(),
                vec!["score".into()],
                vec![0]
            )
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        manager
            .create_edge_index(
                1,
                "edge_score".into(),
                1,
                "link".into(),
                vec!["score".into()],
                vec![0]
            )
            .await
            .unwrap(),
        2
    );
}

impl Fixture {
    async fn new() -> Self {
        let mut stores = Vec::new();
        let mut indexes = Vec::new();
        let mut partitions = Vec::new();
        let mut ports = Vec::new();
        let mut tasks = Vec::new();
        let meta_service = Arc::new(byoridb_meta::MetaService::new(Arc::new(
            MemoryKVStore::new(),
        )));
        for part in 1..=2 {
            let store = Arc::new(MemoryKVStore::new());
            let manager = Arc::new(IndexManager::new(store.clone()));
            create_indexes(&manager).await;
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = u32::from(listener.local_addr().unwrap().port());
            let partition = Arc::new(PartitionManager::new("127.0.0.1".into(), port));
            partition.register_space(1, 2).await;
            partition
                .add_partition_with_status(1, part, PartitionStatus::Leader)
                .await;
            meta_service
                .update_part_allocation(1, part, vec![("127.0.0.1".into(), port)])
                .await
                .unwrap();
            let service = StorageRpcService::with_index_manager(
                partition.clone(),
                store.clone(),
                manager.clone(),
            );
            tasks.push(tokio::spawn(async move {
                tonic::transport::Server::builder()
                    .add_service(StorageServiceServer::new(service))
                    .serve_with_incoming(incoming(listener))
                    .await
                    .unwrap();
            }));
            stores.push(store);
            indexes.push(manager);
            partitions.push(partition);
            ports.push(port);
        }
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = u32::from(listener.local_addr().unwrap().port());
        tasks.push(tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(MetaServiceServer::new(MetaRpcService::new(
                    meta_service,
                    "127.0.0.1".into(),
                    port,
                )))
                .serve_with_incoming(incoming(listener))
                .await
                .unwrap();
        }));
        let meta = Arc::new(MetaClient::new(&format!("127.0.0.1:{port}")).await.unwrap());
        let coordinator = Arc::new(MemoryKVStore::new());
        let coordinator_indexes = Arc::new(IndexManager::new(coordinator.clone()));
        create_indexes(&coordinator_indexes).await;
        for (key, name) in [
            (crate::key::SchemaKey::tag("graph", "item"), "item"),
            (crate::key::SchemaKey::edge("graph", "link"), "link"),
        ] {
            coordinator.put(&key, &serde_json::to_vec(&serde_json::json!({"name": name, "properties": [{"name":"score", "data_type":"Int64"}]})).unwrap()).await.unwrap();
        }
        Self {
            stores,
            indexes,
            partitions,
            ports,
            storage: Arc::new(StorageQueryClient::new()),
            meta,
            coordinator,
            coordinator_indexes,
            tasks,
        }
    }
    fn executor(&self) -> DistributedQueryExecutor {
        DistributedQueryExecutor::new(self.storage.clone(), self.meta.clone())
    }
    fn vids(part: u32, count: usize) -> Vec<i64> {
        (0..)
            .filter(|vid| byoridb_common::hash::compute_partition(*vid, 2) == part)
            .take(count)
            .collect()
    }
    fn key(edge: bool, vid: i64) -> Vec<u8> {
        if edge {
            format!("1:edge:{vid}:link:{}:3", vid + 100_000).into_bytes()
        } else {
            format!("1:vertex:{vid}").into_bytes()
        }
    }
    async fn seed(&self, edge: bool, vid: i64, score: i64) {
        let part = byoridb_common::hash::compute_partition(vid, 2);
        let props = HashMap::from([("score".to_string(), Value::Int(score))]);
        let data = if edge {
            self.indexes[(part - 1) as usize]
                .insert_edge_index(part, 2, &[StoredIndexValue::Int(10)], vid, 3, vid + 100_000)
                .await
                .unwrap();
            VertexCodec::encode_edge(&StoredEdge {
                src_vid: vid,
                dst_vid: vid + 100_000,
                edge_type: "link".into(),
                ranking: 3,
                properties: props,
            })
            .unwrap()
        } else {
            self.indexes[(part - 1) as usize]
                .insert_tag_index(part, 1, &[StoredIndexValue::Int(10)], vid)
                .await
                .unwrap();
            VertexCodec::encode_vertex(&StoredVertex {
                vid,
                tags: vec![TagData {
                    name: "item".into(),
                    properties: props,
                }],
            })
            .unwrap()
        };
        self.stores[(part - 1) as usize]
            .put(&Self::key(edge, vid), &data)
            .await
            .unwrap();
    }
    async fn run(
        &self,
        edge: bool,
        cap: usize,
        memory_mb: usize,
        suffix: &str,
    ) -> crate::error::Result<ExecutorResult> {
        let name = if edge { "link" } else { "item" };
        self.run_sql(
            cap,
            memory_mb,
            &format!("LOOKUP ON {name} WHERE score == 10{suffix}"),
        )
        .await
    }
    async fn run_sql(
        &self,
        cap: usize,
        memory_mb: usize,
        query: &str,
    ) -> crate::error::Result<ExecutorResult> {
        let ctx = ExecutionContext::new(self.coordinator.clone())
            .with_space("graph".into())
            .with_space_id(1)
            .with_index_manager(self.coordinator_indexes.clone())
            .with_meta_client(self.meta.clone())
            .with_distributed_mode(self.storage.clone(), 2)
            .with_config(ExecutionConfig {
                max_scan_limit: cap,
                max_memory_mb: memory_mb,
                ..Default::default()
            });
        let plan = ExecutionPlanBuilder::build(byoridb_parser::parse(query).unwrap()).unwrap();
        Executor::new(Arc::new(ctx)).execute(plan).await
    }
}

fn values() -> Vec<IndexValue> {
    vec![IndexValue {
        value: Some(byoridb_storage::proto::storage::index_value::Value::IntValue(10)),
    }]
}

#[test]
fn invalid_topology_never_becomes_empty_results() {
    let good = |part_id| PartAllocation {
        space_id: 1,
        part_id,
        hosts: vec![("localhost".into(), 9000)],
    };
    assert!(validate_allocations(1, 2, vec![good(2), good(1)]).is_ok());
    for parts in [
        vec![],
        vec![good(1)],
        vec![good(1), good(1)],
        vec![good(0), good(2)],
        vec![good(1), good(3)],
    ] {
        assert!(validate_allocations(1, 2, parts).is_err());
    }
    assert!(validate_allocations(1, 0, vec![]).is_err());
    assert!(validate_allocations(
        0,
        1,
        vec![PartAllocation {
            space_id: 0,
            ..good(1)
        }]
    )
    .is_err());
    for bad in [
        PartAllocation {
            space_id: 2,
            ..good(1)
        },
        PartAllocation {
            hosts: vec![],
            ..good(1)
        },
        PartAllocation {
            hosts: vec![("".into(), 9000)],
            ..good(1)
        },
        PartAllocation {
            hosts: vec![("host".into(), 0)],
            ..good(1)
        },
        PartAllocation {
            hosts: vec![("host".into(), 65536)],
            ..good(1)
        },
    ] {
        assert!(validate_allocations(1, 2, vec![bad, good(2)]).is_err());
    }
}

#[test]
fn malformed_or_repeating_pages_are_errors() {
    let mut seen = HashSet::new();
    assert!(validate_lookup_page(1, 1, true, b"first", &mut seen).is_ok());
    assert!(validate_lookup_page(1, 1, true, b"first", &mut seen).is_err());
    for (len, more, cursor) in [
        (0, true, &b"next"[..]),
        (1, true, &b""[..]),
        (2, false, &b""[..]),
        (1, false, &b"next"[..]),
    ] {
        assert!(validate_lookup_page(len, 1, more, cursor, &mut seen).is_err());
    }
}

#[tokio::test]
async fn distributed_lookup_caps_are_per_partition_not_global_and_limit_is_not_a_budget() {
    let f = Fixture::new().await;
    for edge in [false, true] {
        for part in 1..=2 {
            for vid in Fixture::vids(part, 2) {
                f.seed(edge, vid, 10).await;
            }
        }
        assert_eq!(f.run(edge, 2, 0, "").await.unwrap().rows.len(), 4);
        assert_eq!(
            f.run(edge, 2, 0, " LIMIT 1 OFFSET 1")
                .await
                .unwrap()
                .rows
                .len(),
            1
        );
        let extra = Fixture::vids(1, 3)[2];
        f.seed(edge, extra, 10).await;
        let error = f.run(edge, 2, 0, " LIMIT 1").await.unwrap_err().to_string();
        assert!(error.contains("max_scan_limit"), "{error}");
        assert_eq!(f.run(edge, 0, 0, "").await.unwrap().rows.len(), 5);
    }
}

#[tokio::test]
async fn distributed_lookup_consumes_multiple_pages_and_preserves_skew() {
    let f = Fixture::new().await;
    for edge in [false, true] {
        for vid in Fixture::vids(2, 1030) {
            f.seed(edge, vid, 10).await;
        }
        assert_eq!(f.run(edge, 0, 0, "").await.unwrap().rows.len(), 1030);
        assert_eq!(f.run(edge, 1030, 0, "").await.unwrap().rows.len(), 1030);
    }
}

#[tokio::test]
async fn distributed_lookup_skips_stale_records_but_counts_their_candidates() {
    let f = Fixture::new().await;
    let vids = Fixture::vids(1, 3);
    for edge in [false, true] {
        f.seed(edge, vids[0], 10).await;
        f.stores[0]
            .delete(&Fixture::key(edge, vids[0]))
            .await
            .unwrap();
        f.seed(edge, vids[1], 99).await;
        f.seed(edge, vids[2], 10).await;
        assert_eq!(f.run(edge, 3, 0, " LIMIT 1").await.unwrap().rows.len(), 1);
        assert!(f.run(edge, 2, 0, " LIMIT 1").await.is_err());
    }
}

#[tokio::test]
async fn distributed_lookup_partition_failure_after_success_is_not_partial_success() {
    let f = Fixture::new().await;
    for edge in [false, true] {
        f.seed(edge, Fixture::vids(1, 1)[0], 10).await;
    }
    f.partitions[1].remove_partition(1, 2).await;
    for edge in [false, true] {
        assert!(f.run(edge, 0, 0, " LIMIT 1").await.is_err());
    }
}

#[tokio::test]
async fn distributed_fetch_partial_corruption_and_identity_mismatch_are_errors() {
    let f = Fixture::new().await;
    let vids = vec![Fixture::vids(1, 1)[0], Fixture::vids(2, 1)[0]];
    for edge in [false, true] {
        for vid in &vids {
            f.seed(edge, *vid, 10).await;
        }
        f.stores[1]
            .put(&Fixture::key(edge, vids[1]), b"corrupt")
            .await
            .unwrap();
        if edge {
            let keys = vids
                .iter()
                .map(|vid| EdgeKey {
                    src_vid: *vid,
                    dst_vid: *vid + 100_000,
                    edge_type: "link".into(),
                    ranking: 3,
                })
                .collect();
            assert!(f
                .executor()
                .execute_fetch_edges(1, 2, keys, vec![])
                .await
                .is_err());
        } else {
            assert!(f
                .executor()
                .execute_fetch(1, 2, vids.clone(), vec![], vec![])
                .await
                .is_err());
        }
        assert!(f.run(edge, 0, 0, "").await.is_err());
        let wrong = if edge {
            VertexCodec::encode_edge(&StoredEdge {
                src_vid: vids[1],
                dst_vid: 9,
                edge_type: "link".into(),
                ranking: 3,
                properties: HashMap::new(),
            })
            .unwrap()
        } else {
            VertexCodec::encode_vertex(&StoredVertex {
                vid: vids[1] + 1,
                tags: vec![],
            })
            .unwrap()
        };
        f.stores[1]
            .put(&Fixture::key(edge, vids[1]), &wrong)
            .await
            .unwrap();
        assert!(f.run(edge, 0, 0, "").await.is_err());
    }
}

#[tokio::test]
async fn distributed_lookup_result_budget_applies_to_full_property_projection() {
    let f = Fixture::new().await;
    let vid = Fixture::vids(1, 1)[0];
    f.seed(false, vid, 10).await;
    let data = VertexCodec::encode_vertex(&StoredVertex {
        vid,
        tags: vec![TagData {
            name: "item".into(),
            properties: HashMap::from([
                ("score".into(), Value::Int(10)),
                ("payload".into(), Value::String("x".repeat(1_100_000))),
            ]),
        }],
    })
    .unwrap();
    f.stores[0]
        .put(&Fixture::key(false, vid), &data)
        .await
        .unwrap();
    assert!(matches!(
        f.run(false, 0, 1, "").await,
        Err(crate::error::ExecutionError::ResourceExhausted(_))
    ));
    assert_eq!(f.run(false, 0, 0, "").await.unwrap().rows.len(), 1);
}

#[tokio::test]
async fn distributed_rpc_cursors_are_scoped_and_continue_without_duplicates() {
    let f = Fixture::new().await;
    let vids = Fixture::vids(1, 3);
    for edge in [false, true] {
        for vid in &vids {
            f.seed(edge, *vid, 10).await;
        }
    }
    let first = f
        .storage
        .lookup_tag_index(
            "127.0.0.1",
            f.ports[0],
            1,
            1,
            1,
            "tag_score".into(),
            values(),
            2,
            vec![],
        )
        .await
        .unwrap();
    assert!(first.has_more);
    assert_eq!(first.vids.len(), 2);
    let second = f
        .storage
        .lookup_tag_index(
            "127.0.0.1",
            f.ports[0],
            1,
            1,
            1,
            "tag_score".into(),
            values(),
            2,
            first.next_cursor.clone(),
        )
        .await
        .unwrap();
    assert!(!second.has_more);
    assert_eq!(second.vids.len(), 1);
    assert!(second.next_cursor.is_empty());
    assert!(!first.vids.contains(&second.vids[0]));
    assert!(f
        .storage
        .lookup_tag_index(
            "127.0.0.1",
            f.ports[0],
            1,
            1,
            1,
            "tag_score".into(),
            vec![],
            2,
            first.next_cursor.clone()
        )
        .await
        .is_err());
    assert!(f
        .storage
        .lookup_tag_index(
            "127.0.0.1",
            f.ports[1],
            1,
            2,
            1,
            "tag_score".into(),
            values(),
            2,
            first.next_cursor
        )
        .await
        .is_err());
    let first = f
        .storage
        .lookup_edge_index(
            "127.0.0.1",
            f.ports[0],
            1,
            1,
            2,
            "edge_score".into(),
            values(),
            2,
            vec![],
        )
        .await
        .unwrap();
    assert!(first.has_more);
    let second = f
        .storage
        .lookup_edge_index(
            "127.0.0.1",
            f.ports[0],
            1,
            1,
            2,
            "edge_score".into(),
            values(),
            2,
            first.next_cursor,
        )
        .await
        .unwrap();
    assert!(!second.has_more);
    assert_eq!(second.edges.len(), 1);
    assert!(!first.edges.contains(&second.edges[0]));
}

#[tokio::test]
async fn distributed_lookup_valid_legacy_json_and_zero_vid_remain_supported() {
    let f = Fixture::new().await;
    for (edge, bytes) in [
        (
            false,
            &br#"{"tags":[{"name":"item","props":{"score":10}}]}"#[..],
        ),
        (
            true,
            &br#"{"src":0,"dst":100000,"type":"link","rank":3,"properties":{"score":10}}"#[..],
        ),
    ] {
        f.seed(edge, 0, 10).await;
        let part = byoridb_common::hash::compute_partition(0, 2);
        f.stores[(part - 1) as usize]
            .put(&Fixture::key(edge, 0), bytes)
            .await
            .unwrap();
        assert_eq!(f.run(edge, 0, 0, "").await.unwrap().rows.len(), 1);
    }
}

#[tokio::test]
async fn distributed_lookup_malformed_index_candidates_fail_instead_of_disappearing() {
    use byoridb_storage::key::KeyUtils;
    let f = Fixture::new().await;
    let vid = Fixture::vids(1, 1)[0];
    for edge in [false, true] {
        f.seed(edge, vid, 10).await;
        let mut malformed = if edge {
            KeyUtils::edge_index_key(1, 2, &[StoredIndexValue::Int(10)], vid, 3, vid + 100_000)
        } else {
            KeyUtils::tag_index_key(1, 1, &[StoredIndexValue::Int(10)], vid)
        };
        malformed.push(0);
        f.stores[0].put(&malformed, b"").await.unwrap();
        assert!(f.run(edge, 1, 0, " LIMIT 1").await.is_err());
    }
}

#[tokio::test]
async fn distributed_lookup_rejects_wrong_partition_candidates() {
    let f = Fixture::new().await;
    let vid = Fixture::vids(2, 1)[0];
    f.indexes[0]
        .insert_tag_index(1, 1, &[StoredIndexValue::Int(10)], vid)
        .await
        .unwrap();
    f.indexes[0]
        .insert_edge_index(1, 2, &[StoredIndexValue::Int(10)], vid, 3, vid + 100_000)
        .await
        .unwrap();
    for edge in [false, true] {
        let error = f.run(edge, 0, 0, "").await.unwrap_err().to_string();
        assert!(error.contains("another partition"), "{error}");
    }
}

#[tokio::test]
async fn distributed_lookup_malformed_legacy_fields_fail_closed() {
    let f = Fixture::new().await;
    let vid = Fixture::vids(1, 1)[0];
    for edge in [false, true] {
        f.seed(edge, vid, 10).await;
        let malformed = if edge {
            serde_json::json!({"src":vid,"dst":vid+100_000,"type":"link","rank":3,"properties":[]})
        } else {
            serde_json::json!({"vid":vid,"tags":[{"name":"item","props":[]}]})
        };
        f.stores[0]
            .put(
                &Fixture::key(edge, vid),
                &serde_json::to_vec(&malformed).unwrap(),
            )
            .await
            .unwrap();
        assert!(f.run(edge, 0, 0, "").await.is_err());
    }
}

#[tokio::test]
async fn distributed_rpc_keyset_cursor_survives_deletion_of_previous_row() {
    use byoridb_storage::key::KeyUtils;
    let f = Fixture::new().await;
    for vid in Fixture::vids(1, 3) {
        f.seed(false, vid, 10).await;
    }
    let first = f
        .storage
        .lookup_tag_index(
            "127.0.0.1",
            f.ports[0],
            1,
            1,
            1,
            "tag_score".into(),
            values(),
            1,
            vec![],
        )
        .await
        .unwrap();
    assert!(first.has_more);
    f.stores[0]
        .delete(&KeyUtils::tag_index_key(
            1,
            1,
            &[StoredIndexValue::Int(10)],
            first.vids[0],
        ))
        .await
        .unwrap();
    let second = f
        .storage
        .lookup_tag_index(
            "127.0.0.1",
            f.ports[0],
            1,
            1,
            1,
            "tag_score".into(),
            values(),
            2,
            first.next_cursor,
        )
        .await
        .unwrap();
    assert_eq!(second.vids.len(), 2);
    assert!(!second.has_more);
    assert!(second.next_cursor.is_empty());
}

#[tokio::test]
async fn distributed_scan_continues_real_ranges_and_propagates_partition_failure() {
    let f = Fixture::new().await;
    for vid in Fixture::vids(1, 1030) {
        f.seed(false, vid, 10).await;
    }
    assert_eq!(
        f.executor()
            .execute_scan(1, 2, "item", vec![], 0)
            .await
            .unwrap()
            .len(),
        1030
    );
    assert_eq!(
        f.executor()
            .execute_scan(1, 2, "item", vec![], 1025)
            .await
            .unwrap()
            .len(),
        1025
    );
    f.partitions[1].remove_partition(1, 2).await;
    assert!(f
        .executor()
        .execute_scan(1, 2, "item", vec![], 1)
        .await
        .is_err());
}

#[tokio::test]
async fn distributed_scan_corrupt_records_cannot_become_no_match() {
    let f = Fixture::new().await;
    let vid = Fixture::vids(1, 1)[0];
    f.seed(false, vid, 10).await;
    f.stores[0]
        .put(&Fixture::key(false, vid), b"corrupt")
        .await
        .unwrap();
    assert!(f
        .executor()
        .execute_scan(1, 2, "item", vec![], 0)
        .await
        .is_err());
}

#[tokio::test]
async fn distributed_go_partial_partition_failure_is_an_error() {
    let f = Fixture::new().await;
    let vids = vec![Fixture::vids(1, 1)[0], Fixture::vids(2, 1)[0]];
    for vid in &vids {
        f.seed(true, *vid, 10).await;
    }
    f.partitions[1].remove_partition(1, 2).await;
    assert!(f
        .executor()
        .execute_go(1, 2, vids, "link", vec![])
        .await
        .is_err());
}

#[tokio::test]
async fn distributed_go_corruption_cannot_become_partial_success() {
    for corrupt_key in [false, true] {
        let f = Fixture::new().await;
        let vids = Fixture::vids(1, 2);
        for vid in &vids {
            f.seed(true, *vid, 10).await;
        }
        let key = if corrupt_key {
            format!("1:edge:{}:link:not-a-vid:3", vids[1]).into_bytes()
        } else {
            Fixture::key(true, vids[1])
        };
        f.stores[0].put(&key, b"corrupt").await.unwrap();
        assert!(f
            .executor()
            .execute_go(1, 2, vids, "link", vec![])
            .await
            .is_err());
    }
}

#[tokio::test]
async fn distributed_rpc_rejects_mismatched_index_id_and_name() {
    let f = Fixture::new().await;
    assert!(f
        .storage
        .lookup_tag_index(
            "127.0.0.1",
            f.ports[0],
            1,
            1,
            1,
            "wrong_name".into(),
            values(),
            2,
            vec![]
        )
        .await
        .is_err());
    assert!(f
        .storage
        .lookup_edge_index(
            "127.0.0.1",
            f.ports[0],
            1,
            1,
            2,
            "wrong_name".into(),
            values(),
            2,
            vec![]
        )
        .await
        .is_err());
}

#[tokio::test]
async fn distributed_fetch_rejects_lost_ownership_and_transferring_partitions() {
    let f = Fixture::new().await;
    let vid = Fixture::vids(1, 1)[0];
    for edge in [false, true] {
        f.seed(edge, vid, 10).await;
    }
    f.partitions[0]
        .add_partition_with_status(1, 1, PartitionStatus::Transferring)
        .await;
    assert!(f
        .executor()
        .execute_fetch(1, 2, vec![vid], vec![], vec![])
        .await
        .is_err());
    assert!(f.run(false, 0, 0, "").await.is_err());
    f.partitions[0].remove_partition(1, 1).await;
    assert!(f
        .executor()
        .execute_fetch_edges(
            1,
            2,
            vec![EdgeKey {
                src_vid: vid,
                dst_vid: vid + 100_000,
                edge_type: "link".into(),
                ranking: 3
            }],
            vec![]
        )
        .await
        .is_err());
}

#[tokio::test]
async fn distributed_float_equality_matches_scan_semantics_for_epsilon_and_signed_zero() {
    use byoridb_storage::key::KeyUtils;
    let f = Fixture::new().await;
    let samples = [
        Value::Float(0.0),
        Value::Float(-0.0),
        Value::Float(1e-10),
        Value::Float(1.0),
        Value::Float(1.0 + 1e-10),
        Value::Float(1.0 + 1e-6),
        Value::Int(0),
        Value::Int(1),
    ];
    for edge in [false, true] {
        for (vid, value) in (0..).zip(samples.iter()) {
            f.seed(edge, vid, 10).await;
            let part = byoridb_common::hash::compute_partition(vid, 2);
            let store = &f.stores[(part - 1) as usize];
            let index = &f.indexes[(part - 1) as usize];
            let indexed = match value {
                Value::Float(v) => StoredIndexValue::Float(*v),
                Value::Int(v) => StoredIndexValue::Int(*v),
                _ => unreachable!(),
            };
            let props = HashMap::from([("score".into(), value.clone())]);
            let data = if edge {
                store
                    .delete(&KeyUtils::edge_index_key(
                        part,
                        2,
                        &[StoredIndexValue::Int(10)],
                        vid,
                        3,
                        vid + 100_000,
                    ))
                    .await
                    .unwrap();
                index
                    .insert_edge_index(part, 2, &[indexed], vid, 3, vid + 100_000)
                    .await
                    .unwrap();
                VertexCodec::encode_edge(&StoredEdge {
                    src_vid: vid,
                    dst_vid: vid + 100_000,
                    edge_type: "link".into(),
                    ranking: 3,
                    properties: props,
                })
                .unwrap()
            } else {
                store
                    .delete(&KeyUtils::tag_index_key(
                        part,
                        1,
                        &[StoredIndexValue::Int(10)],
                        vid,
                    ))
                    .await
                    .unwrap();
                index
                    .insert_tag_index(part, 1, &[indexed], vid)
                    .await
                    .unwrap();
                VertexCodec::encode_vertex(&StoredVertex {
                    vid,
                    tags: vec![TagData {
                        name: "item".into(),
                        properties: props,
                    }],
                })
                .unwrap()
            };
            store.put(&Fixture::key(edge, vid), &data).await.unwrap();
        }
        let name = if edge { "link" } else { "item" };
        for (literal, expected) in [("0.0", 3), ("1.0", 2), ("1", 1)] {
            assert_eq!(
                f.run_sql(0, 0, &format!("LOOKUP ON {name} WHERE score == {literal}"))
                    .await
                    .unwrap()
                    .rows
                    .len(),
                expected,
                "{name} {literal}"
            );
        }
        assert!(matches!(
            f.run_sql(
                1,
                0,
                &format!("LOOKUP ON {name} WHERE score == 0.0 LIMIT 1")
            )
            .await,
            Err(crate::error::ExecutionError::ResourceExhausted(_))
        ));
    }
}

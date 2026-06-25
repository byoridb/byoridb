// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! End-to-end: write space/tag/edge metadata + CSVs, run the loader, and assert
//! the exact KV set the engine's INSERT path would produce — vertex blob,
//! tag-vid index, forward edge, reverse edge — plus codec round-trip.

use byoridb_bulkloader::key;
use byoridb_bulkloader::loader::{Loader, LoaderConfig};
use byoridb_bulkloader::schema::{column_types_from_schema, ColumnTypes};
use byoridb_codec::vertex::VertexCodec;
use byoridb_common::Value;
use byoridb_kvstore::{KVStore, KVStoreOptions, RedbKVStore};
use std::path::Path;

fn open_store(dir: &Path) -> RedbKVStore {
    RedbKVStore::open(
        dir,
        KVStoreOptions {
            create_if_missing: true,
            cache_size: 64 * 1024 * 1024,
            use_fsync: false,
        },
    )
    .unwrap()
}

async fn write_meta(store: &RedbKVStore) {
    store
        .put(
            &key::space("s"),
            br#"{"name":"s","id":1,"vid_type":"INT64"}"#,
        )
        .await
        .unwrap();
    store
        .put(
            &key::tag("s", "sku"),
            br#"{"name":"sku","properties":[{"name":"price","data_type":"Int64","nullable":true}]}"#,
        )
        .await
        .unwrap();
    store
        .put(
            &key::edge("s", "same_as"),
            br#"{"name":"same_as","properties":[]}"#,
        )
        .await
        .unwrap();
}

fn cfg() -> LoaderConfig {
    LoaderConfig {
        space: "s".to_string(),
        batch_size: 4,
        id_column: "id".to_string(),
        src_column: "src".to_string(),
        dst_column: "dst".to_string(),
        ranking_column: None,
        strict: false,
    }
}

async fn tag_types(store: &RedbKVStore, name: &str) -> ColumnTypes {
    let bytes = store.get(&key::tag("s", name)).await.unwrap().unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    column_types_from_schema(&json)
}

#[tokio::test]
async fn loads_nodes_edges_with_full_kv_set() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_store(dir.path());
    write_meta(&store).await;

    let node_csv = dir.path().join("sku.csv");
    std::fs::write(&node_csv, "id,price\nS1,100\nS2,200\nS3,300\n").unwrap();
    let edge_csv = dir.path().join("same_as.csv");
    std::fs::write(&edge_csv, "src,dst\nS1,S2\nS2,S3\n").unwrap();

    let types = tag_types(&store, "sku").await;
    let mut ldr = Loader::new(&store, cfg());
    ldr.load_node_file("sku", &node_csv, &types).await.unwrap();
    ldr.load_edge_file("same_as", &edge_csv, &ColumnTypes::new())
        .await
        .unwrap();
    let stats = ldr.finish().await.unwrap();

    assert_eq!(stats.vertices, 3);
    assert_eq!(stats.tagvid_entries, 3);
    assert_eq!(stats.edges, 2);
    assert_eq!(stats.dangling_edges, 0);

    // vid is assigned sequentially in read order: S1=1, S2=2, S3=3.
    let blob = store.get(&key::vertex("s", 1)).await.unwrap().unwrap();
    let v = VertexCodec::decode_vertex(&blob).unwrap();
    assert_eq!(v.vid, 1);
    assert_eq!(v.tags.len(), 1);
    assert_eq!(v.tags[0].name, "sku");
    // Declared Int64 column parsed as Int, original id preserved as String.
    assert!(matches!(
        v.tags[0].properties.get("price"),
        Some(Value::Int(100))
    ));
    assert!(matches!(
        v.tags[0].properties.get("id"),
        Some(Value::String(s)) if s == "S1"
    ));

    // tag-vid index present for label-only MATCH.
    assert!(store
        .get(&key::tagvid("s", "sku", 1))
        .await
        .unwrap()
        .is_some());

    // Forward edge S1->S2 and its reverse in-edge, same payload.
    let fwd = store
        .get(&key::edge_data("s", 1, "same_as", 2, 0))
        .await
        .unwrap()
        .unwrap();
    let rev = store
        .get(&key::in_edge_data("s", 2, "same_as", 1, 0))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fwd, rev, "forward and reverse edge payloads must match");
    let e = VertexCodec::decode_edge(&fwd).unwrap();
    assert_eq!(
        (e.src_vid, e.dst_vid, e.edge_type.as_str()),
        (1, 2, "same_as")
    );
}

#[tokio::test]
async fn dangling_edge_is_dropped_in_lenient_mode() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_store(dir.path());
    write_meta(&store).await;

    let node_csv = dir.path().join("sku.csv");
    std::fs::write(&node_csv, "id,price\nS1,1\n").unwrap();
    let edge_csv = dir.path().join("same_as.csv");
    // S9 is never loaded → dangling.
    std::fs::write(&edge_csv, "src,dst\nS1,S9\n").unwrap();

    let mut ldr = Loader::new(&store, cfg());
    ldr.load_node_file("sku", &node_csv, &ColumnTypes::new())
        .await
        .unwrap();
    ldr.load_edge_file("same_as", &edge_csv, &ColumnTypes::new())
        .await
        .unwrap();
    let stats = ldr.finish().await.unwrap();

    assert_eq!(stats.edges, 0);
    assert_eq!(stats.dangling_edges, 1);
}

#[tokio::test]
async fn reserved_sameas_edge_type_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_store(dir.path());
    write_meta(&store).await;
    let edge_csv = dir.path().join("x.csv");
    std::fs::write(&edge_csv, "src,dst\nS1,S2\n").unwrap();

    let mut ldr = Loader::new(&store, cfg());
    let err = ldr
        .load_edge_file("sameAs", &edge_csv, &ColumnTypes::new())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("same_as"));
}

#[tokio::test]
async fn strict_mode_aborts_on_duplicate_id() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_store(dir.path());
    write_meta(&store).await;
    let node_csv = dir.path().join("sku.csv");
    std::fs::write(&node_csv, "id,price\nS1,1\nS1,2\n").unwrap();

    let mut c = cfg();
    c.strict = true;
    let mut ldr = Loader::new(&store, c);
    let err = ldr
        .load_node_file("sku", &node_csv, &ColumnTypes::new())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("duplicate"));
}

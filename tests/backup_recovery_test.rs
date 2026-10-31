//! Offline CLI recovery drills exercise persisted data and failed restores.
use byoridb_common::Value;
use byoridb_graph::{AuthManager, GraphService};
use byoridb_kvstore::{BackupManager, KVStore, KVStoreOptions, RedbKVStore};
use std::{
    path::Path,
    process::{Command, Output},
    sync::Arc,
    time::Duration,
};

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_byoridb-backup"))
        .args(args)
        .output()
        .unwrap()
}
fn successful(output: Output) {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
fn service(store: Arc<dyn KVStore>) -> GraphService {
    GraphService::with_auth(
        store,
        AuthManager::with_config("backup-root-password", Duration::from_secs(3600)),
    )
}
fn open(path: &Path) -> Arc<RedbKVStore> {
    Arc::new(RedbKVStore::open(path, KVStoreOptions::default()).unwrap())
}

#[tokio::test]
async fn cli_round_trip_preserves_graph_schema_indexes_users_string_vids_and_history() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let archives = temp.path().join("archives");
    let target = temp.path().join("recovered");
    let store = open(&source);
    let graph = service(store.clone());
    let root = graph
        .authenticate("root".into(), "backup-root-password".into())
        .await
        .unwrap();
    for query in [
        "CREATE SPACE recovery (vid_type=FIXED_STRING(32))",
        "USE recovery",
        "CREATE TAG person(name STRING)",
        "CREATE EDGE knows(since INT64)",
        "CREATE TAG INDEX person_name ON person(name)",
        r#"INSERT VERTEX person(name) VALUES "alice":("Alice"), "bob":("Bob")"#,
        r#"INSERT EDGE knows(since) VALUES "alice"->"bob":(2024)"#,
        r#"CREATE USER recovery_reader WITH PASSWORD "reader-password" ROLE GUEST"#,
        "CREATE SPACE history_space",
        "USE history_space",
        "CREATE TAG note(body STRING)",
        r#"INSERT VERTEX note(body) VALUES 1:("before")"#,
        r#"UPDATE VERTEX ON note 1 SET body = "after""#,
        "DELETE VERTEX 1",
    ] {
        graph
            .execute(root, query.into())
            .await
            .unwrap_or_else(|e| panic!("{query}: {e}"));
    }
    let history = store.scan_history(b"history_space:vertex:1").await.unwrap();
    assert_eq!(history.len(), 3);
    assert!(
        history[0].value.is_empty(),
        "deleted vertex must have a tombstone"
    );
    let query_history = format!("FETCH PROP ON note 1 AS OF {}", history[2].tx);
    let expected_past = graph.execute(root, query_history.clone()).await.unwrap();
    assert_eq!(expected_past.rows.len(), 1);
    assert!(matches!(&expected_past.rows[0][1], Value::String(s) if s.contains("before")));
    let current = graph
        .execute(root, "FETCH PROP ON note 1".into())
        .await
        .unwrap();
    assert!(current.rows.is_empty());
    graph.execute(root, "USE recovery".into()).await.unwrap();
    let graph_query = r#"MATCH (a:person)-[:knows]->(b:person) RETURN id(a) AS source, a.person.name AS name, id(b) AS target"#;
    let expected_graph = graph.execute(root, graph_query.into()).await.unwrap();
    assert_eq!(
        expected_graph.rows,
        vec![vec![
            Value::String("alice".into()),
            Value::String("Alice".into()),
            Value::String("bob".into())
        ]]
    );
    let index_query = r#"LOOKUP ON person WHERE person.name == "Bob" YIELD person.name"#;
    let expected_index = graph.execute(root, index_query.into()).await.unwrap();
    assert_eq!(expected_index.rows.len(), 1);
    let before_kv = store.scan_prefix(b"").await.unwrap();
    let entities = store.scan_history_entity_keys(b"").await.unwrap();
    let mut before_history = Vec::new();
    for entity in entities {
        before_history.push((entity.clone(), store.scan_history(&entity).await.unwrap()));
    }
    drop(graph);
    drop(store);

    successful(cli(&[
        "create",
        "--db",
        source.to_str().unwrap(),
        "--backup-dir",
        archives.to_str().unwrap(),
        "--label",
        "recovery-drill",
    ]));
    let manager = BackupManager::new(&source, &archives).unwrap();
    let backups = manager.list_backups().unwrap();
    assert_eq!(backups.len(), 1);
    let id = &backups[0].id;
    let archive_data = archives.join(id).join("data.redb");
    let immutable_bytes = std::fs::read(&archive_data).unwrap();
    successful(cli(&[
        "verify",
        "--backup-dir",
        archives.to_str().unwrap(),
        "--backup-id",
        id,
    ]));
    assert_eq!(std::fs::read(&archive_data).unwrap(), immutable_bytes);
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("sentinel"), b"existing target").unwrap();
    assert!(!cli(&[
        "restore",
        "--backup-dir",
        archives.to_str().unwrap(),
        "--backup-id",
        id,
        "--target",
        target.to_str().unwrap(),
    ])
    .status
    .success());
    assert_eq!(
        std::fs::read(target.join("sentinel")).unwrap(),
        b"existing target"
    );
    successful(cli(&[
        "restore",
        "--backup-dir",
        archives.to_str().unwrap(),
        "--backup-id",
        id,
        "--target",
        target.to_str().unwrap(),
        "--overwrite",
    ]));
    assert!(!target.join("sentinel").exists());
    assert_eq!(std::fs::read(&archive_data).unwrap(), immutable_bytes);

    let restored = open(&target);
    assert_eq!(
        restored.scan_prefix(b"").await.unwrap(),
        before_kv,
        "all schema/index/auth/VID mappings must be byte-exact"
    );
    for (entity, versions) in before_history {
        assert_eq!(restored.scan_history(&entity).await.unwrap(), versions);
    }
    let graph = service(restored);
    assert_eq!(graph.hydrate_persisted_users().await.unwrap(), 1);
    let reader = graph
        .authenticate("recovery_reader".into(), "reader-password".into())
        .await
        .unwrap();
    graph.execute(reader, "USE recovery".into()).await.unwrap();
    assert_eq!(
        graph
            .execute(reader, graph_query.into())
            .await
            .unwrap()
            .rows,
        expected_graph.rows
    );
    assert_eq!(
        graph
            .execute(reader, index_query.into())
            .await
            .unwrap()
            .rows,
        expected_index.rows
    );
    graph
        .execute(reader, "USE history_space".into())
        .await
        .unwrap();
    assert_eq!(
        graph.execute(reader, query_history).await.unwrap().rows,
        expected_past.rows
    );
    assert!(graph
        .execute(reader, "FETCH PROP ON note 1".into())
        .await
        .unwrap()
        .rows
        .is_empty());
}

#[test]
fn cli_missing_corrupt_and_metadata_only_archives_fail_without_deleting_target() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    drop(open(&source));
    let archives = temp.path().join("archives");
    let manager = BackupManager::new(&source, &archives).unwrap();
    for mode in ["missing", "truncated", "corrupt"] {
        let info = manager.create_backup(None).unwrap();
        let data = archives.join(&info.id).join("data.redb");
        match mode {
            "missing" => std::fs::remove_file(data).unwrap(),
            "truncated" => std::fs::write(data, b"not a redb database").unwrap(),
            _ => {
                let mut bytes = std::fs::read(&data).unwrap();
                bytes[0] ^= 0xff;
                std::fs::write(data, bytes).unwrap();
            }
        }
        let target = temp.path().join(format!("target-{mode}"));
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("sentinel"), b"retain me").unwrap();
        let verified = cli(&[
            "verify",
            "--backup-dir",
            archives.to_str().unwrap(),
            "--backup-id",
            &info.id,
        ]);
        assert!(!verified.status.success(), "verify must reject {mode}");
        let restored = cli(&[
            "restore",
            "--backup-dir",
            archives.to_str().unwrap(),
            "--backup-id",
            &info.id,
            "--target",
            target.to_str().unwrap(),
            "--overwrite",
        ]);
        assert!(!restored.status.success(), "restore must reject {mode}");
        assert_eq!(
            std::fs::read(target.join("sentinel")).unwrap(),
            b"retain me"
        );
        assert!(!target.join("data.redb").exists());
    }
    assert!(
        !cli(&["verify", "--backup-dir", archives.to_str().unwrap()])
            .status
            .success()
    );
}

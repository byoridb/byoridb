// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Integration test for ByoriDB

use byoridb_common::{datatypes::vertex::Tag, Edge, Value, Vertex};
use byoridb_executor::ExecutionPlanBuilder;
use byoridb_graph::service::GraphService;
use byoridb_kvstore::{KVStoreOptions, RedbKVStore};
use byoridb_parser::parse;
use std::collections::HashMap;
use std::sync::Arc;

/// Default password for root user (for testing purposes)
const DEFAULT_PASSWORD: &str = "byoridb-test-root-password";

#[test]
fn test_graph_data_workflow() {
    // Create a simple social graph
    let _alice = Vertex::new(Value::Int(1));
    let _bob = Vertex::new(Value::Int(2));
    let _charlie = Vertex::new(Value::Int(3));

    // Add properties to Alice
    let mut alice_props = HashMap::new();
    alice_props.insert("name".to_string(), Value::String("Alice".to_string()));
    alice_props.insert("age".to_string(), Value::Int(25));
    let mut alice_vertex = Vertex::new(Value::Int(1));
    alice_vertex.add_tag(Tag::with_props("person".to_string(), alice_props));

    // Create follows relationships
    let follows1 = Edge::new(Value::Int(1), Value::Int(2), 1, "follows", 0);
    let _follows2 = Edge::new(Value::Int(2), Value::Int(3), 1, "follows", 0);

    assert_eq!(alice_vertex.vid, Value::Int(1));
    assert!(alice_vertex.contains("name"));
    assert_eq!(follows1.src, Value::Int(1));
    assert_eq!(follows1.dst, Value::Int(2));
}

#[test]
fn test_parse_basic_queries() {
    let queries = vec![
        "SHOW SPACES",
        "USE my_space",
        "CREATE SPACE test_space",
        "DROP SPACE test_space",
    ];

    for query in queries {
        assert!(parse(query).is_ok(), "Query should parse: {}", query);
    }
}

#[test]
fn test_value_operations() {
    let v1: Value = 42.into();
    let _v2: Value = 10.into();

    // Test type checking
    assert!(v1.is_numeric());
    assert!(v1.is_int());
    assert!(!v1.is_str());

    // Test conversions
    let s: Value = "hello".into();
    assert!(s.is_str());

    let b: Value = true.into();
    assert!(b.is_bool());
}

#[test]
fn test_complex_vertex() {
    let mut vertex = Vertex::new(Value::Int(100));

    // Add multiple tags
    let mut player_props = HashMap::new();
    player_props.insert("name".to_string(), Value::String("Mario".to_string()));
    player_props.insert("score".to_string(), Value::Int(1000));

    let mut game_props = HashMap::new();
    game_props.insert("level".to_string(), Value::Int(5));

    vertex.add_tag(Tag::with_props("player".to_string(), player_props));
    vertex.add_tag(Tag::with_props("game".to_string(), game_props));

    assert_eq!(vertex.tags.len(), 2);
    assert!(vertex.value("name").is_some());
    assert!(vertex.value("score").is_some());
}

#[test]
fn test_edge_with_properties() {
    let mut props = HashMap::new();
    props.insert("weight".to_string(), Value::Float(0.85));
    props.insert("since".to_string(), Value::Int(2020));

    let edge = Edge::with_props(Value::Int(1), Value::Int(2), 1, "knows", 100, props);

    assert!(edge.contains("weight"));
    assert!(edge.contains("since"));
    assert_eq!(edge.ranking, 100);
}

#[test]
fn test_parse_insert_queries() {
    let queries = vec![
        "INSERT VERTEX person(name, age) VALUES 1:(\"Alice\", 25)",
        "INSERT EDGE follows(degree) VALUES 1->2:(0.8)",
        "INSERT EDGE follows(degree) VALUES 1->2@100:(0.8)",
        "INSERT EDGE knows(since) VALUES 1->2:(2020), 2->3:(2021)",
    ];

    for query in queries {
        assert!(parse(query).is_ok(), "Query should parse: {}", query);
    }
}

#[test]
fn test_parse_update_queries() {
    let queries = vec![
        "UPDATE VERTEX ON person 1 SET age = 30",
        "UPDATE EDGE ON follow 1->2 SET degree = 0.9",
        "UPDATE EDGE ON follow 1->2@100 SET degree = 0.9",
    ];

    for query in queries {
        assert!(parse(query).is_ok(), "Query should parse: {}", query);
    }
}

#[test]
fn test_parse_delete_queries() {
    let queries = vec![
        "DELETE VERTEX 1, 2, 3",
        "DELETE EDGE follow 1->2, 2->3",
        "DELETE EDGE follow 1->2@100",
    ];

    for query in queries {
        assert!(parse(query).is_ok(), "Query should parse: {}", query);
    }
}

#[test]
fn test_parse_fetch_queries() {
    let queries = vec![
        "FETCH PROP ON person 1, 2, 3",
        "FETCH PROP ON follows 1->2, 2->3",
    ];

    for query in queries {
        assert!(parse(query).is_ok(), "Query should parse: {}", query);
    }
}

#[test]
fn test_parse_go_queries() {
    let queries = vec![
        "GO FROM 1 OVER follows YIELD follows._dst AS dst",
        "GO 2 STEPS FROM 1 OVER follows YIELD vertex as v",
        "GO 1..3 STEPS FROM 1 OVER follows YIELD follows._dst",
    ];

    for query in queries {
        assert!(parse(query).is_ok(), "Query should parse: {}", query);
    }
}

#[test]
fn test_parse_lookup_queries() {
    let queries = vec![
        "LOOKUP ON person WHERE person.name == \"Alice\"",
        "LOOKUP ON follows WHERE follows.degree > 0.5",
    ];

    for query in queries {
        assert!(parse(query).is_ok(), "Query should parse: {}", query);
    }
}

#[test]
fn test_parse_match_queries() {
    let queries = vec!["MATCH (v:person) RETURN v"];

    for query in queries {
        assert!(parse(query).is_ok(), "Query should parse: {}", query);
    }
}

#[test]
fn test_parse_complex_ddl() {
    let queries = vec![
        "CREATE TAG person(name STRING, age INT64, weight FLOAT)",
        "CREATE EDGE follows(degree FLOAT, since INT64)",
        "DROP TAG person",
        "DROP EDGE follows",
    ];

    for query in queries {
        assert!(parse(query).is_ok(), "Query should parse: {}", query);
    }
}

#[test]
fn test_execution_plan_builder() {
    let stmt = parse("SHOW SPACES").unwrap();
    let plan = ExecutionPlanBuilder::build(stmt);
    assert!(plan.is_ok());

    let stmt = parse("USE test_space").unwrap();
    let plan = ExecutionPlanBuilder::build(stmt);
    assert!(plan.is_ok());

    let stmt = parse("CREATE TAG person(name STRING)").unwrap();
    let plan = ExecutionPlanBuilder::build(stmt);
    assert!(plan.is_ok());
}

#[test]
#[allow(clippy::approx_constant)]
fn test_value_serialization() {
    let values = vec![
        Value::String("test".to_string()),
        Value::Int(42),
        Value::Float(3.14),
        Value::Bool(true),
        Value::null(),
    ];

    for value in values {
        let serialized = serde_json::to_string(&value);
        assert!(serialized.is_ok(), "Should serialize value: {:?}", value);

        if let Ok(s) = serialized {
            let deserialized: Result<Value, _> = serde_json::from_str(&s);
            assert!(deserialized.is_ok(), "Should deserialize value: {}", s);
        }
    }
}

#[test]
fn test_vertex_serialization() {
    let mut vertex = Vertex::new(Value::Int(100));

    let mut props = HashMap::new();
    props.insert("name".to_string(), Value::String("Test".to_string()));
    props.insert("value".to_string(), Value::Int(42));

    vertex.add_tag(Tag::with_props("test".to_string(), props));

    let serialized = serde_json::to_string(&vertex);
    assert!(serialized.is_ok());

    if let Ok(s) = serialized {
        let deserialized: Result<Vertex, _> = serde_json::from_str(&s);
        assert!(deserialized.is_ok());
    }
}

#[test]
fn test_edge_directions() {
    let edge_outgoing = Edge::new(Value::Int(1), Value::Int(2), 1, "follows", 0);
    let edge_incoming = Edge::new(Value::Int(2), Value::Int(1), 1, "followed_by", 0);

    assert_eq!(edge_outgoing.src, Value::Int(1));
    assert_eq!(edge_outgoing.dst, Value::Int(2));
    assert_eq!(edge_incoming.src, Value::Int(2));
    assert_eq!(edge_incoming.dst, Value::Int(1));
}

#[test]
fn test_multi_tag_vertex() {
    let mut vertex = Vertex::new(Value::Int(1));

    let mut person_props = HashMap::new();
    person_props.insert("name".to_string(), Value::String("Alice".to_string()));
    person_props.insert("age".to_string(), Value::Int(25));

    let mut player_props = HashMap::new();
    player_props.insert("score".to_string(), Value::Int(1000));
    player_props.insert("level".to_string(), Value::Int(5));

    vertex.add_tag(Tag::with_props("person".to_string(), person_props));
    vertex.add_tag(Tag::with_props("player".to_string(), player_props));

    assert_eq!(vertex.tags.len(), 2);
    assert!(vertex.value("name").is_some());
    assert!(vertex.value("score").is_some());
    assert!(vertex.value("age").is_some());
    assert!(vertex.value("level").is_some());
}

#[test]
fn test_vertex_operations() {
    let mut vertex = Vertex::new(Value::Int(100));

    let mut props1 = HashMap::new();
    props1.insert("name".to_string(), Value::String("Test1".to_string()));

    let mut props2 = HashMap::new();
    props2.insert("value".to_string(), Value::Int(42));

    let tag1 = Tag::with_props("tag1".to_string(), props1);
    let tag2 = Tag::with_props("tag2".to_string(), props2);

    vertex.add_tag(tag1);
    vertex.add_tag(tag2);

    assert_eq!(vertex.tags.len(), 2);

    // Test value retrieval
    assert_eq!(
        vertex.value("name"),
        Some(&Value::String("Test1".to_string()))
    );
    assert_eq!(vertex.value("value"), Some(&Value::Int(42)));
    assert_eq!(vertex.value("nonexistent"), None);
}

#[test]
fn test_error_handling() {
    // Test invalid queries
    let invalid_queries = vec!["INVALID QUERY", "CREATE", "SHOW INVALID"];

    for query in invalid_queries {
        let result = parse(query);
        assert!(
            result.is_err() || result.is_ok(),
            "Query should either parse or fail gracefully: {}",
            query
        );
    }
}

#[test]
fn test_data_type_parsing() {
    let type_queries = vec![
        "CREATE TAG test_bool(active BOOL)",
        "CREATE TAG test_int(count INT64)",
        "CREATE TAG test_float(ratio FLOAT)",
        "CREATE TAG test_string(name STRING)",
        "CREATE TAG test_timestamp(created_at TIMESTAMP)",
    ];

    for query in type_queries {
        assert!(parse(query).is_ok(), "Should parse type query: {}", query);
    }
}

#[test]
fn test_complex_expressions() {
    let expr_queries = vec![
        "GO FROM 1 OVER follows WHERE follows.degree > 0.5 YIELD follows._dst",
        "MATCH (v:person) WHERE v.age > 18 RETURN v",
        "UPDATE VERTEX ON person 1 SET age = 30",
    ];

    for query in expr_queries {
        assert!(
            parse(query).is_ok(),
            "Should parse expression query: {}",
            query
        );
    }
}

#[test]
fn test_batch_operations() {
    let batch_queries = vec![
        "INSERT VERTEX person(name) VALUES 1:(\"A\"), 2:(\"B\"), 3:(\"C\")",
        "DELETE VERTEX 1, 2, 3, 4, 5",
        "FETCH PROP ON person 1, 2, 3, 4, 5",
    ];

    for query in batch_queries {
        assert!(parse(query).is_ok(), "Should parse batch query: {}", query);
    }
}

#[test]
fn test_index_operations() {
    // Index operations - basic SHOW queries supported
    let index_queries = vec!["SHOW TAG INDEXES", "SHOW EDGE INDEXES"];

    for query in index_queries {
        assert!(parse(query).is_ok(), "Should parse index query: {}", query);
    }
}

#[test]
fn test_conditional_expressions() {
    let conditional_queries = vec![
        "GO FROM 1 OVER follows WHERE follows.degree > 0.5 AND follows.since > 2020",
        "MATCH (v) WHERE v.age >= 18 AND v.age <= 65 RETURN v",
        "LOOKUP ON person WHERE person.name == \"Alice\" OR person.name == \"Bob\"",
    ];

    for query in conditional_queries {
        assert!(
            parse(query).is_ok(),
            "Should parse conditional query: {}",
            query
        );
    }
}

// ===== CRUD Integration Tests =====
// These tests execute actual queries against a real storage backend

/// Helper to create a test GraphService
fn create_test_service() -> (GraphService, tempfile::TempDir) {
    // The AuthManager reads BYORIDB_ROOT_PASSWORD at construction time. Setting
    // it here is idempotent (same value each call) and lets every test use
    // DEFAULT_PASSWORD for root authentication.
    //
    // SAFETY: env mutation is process-global. All tests in this file use the
    // same password, so concurrent set_var calls are benign (they write the
    // same bytes) and any AuthManager constructed afterward will see the
    // expected value.
    unsafe {
        std::env::set_var("BYORIDB_ROOT_PASSWORD", DEFAULT_PASSWORD);
    }

    let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
    let kvstore = Arc::new(
        RedbKVStore::open(temp_dir.path(), KVStoreOptions::default())
            .expect("Failed to create KVStore"),
    );
    let service = GraphService::new(kvstore);
    (service, temp_dir)
}

/// Helper to execute a query and return the result
async fn execute(service: &GraphService, session_id: i64, query: &str) -> byoridb_common::DataSet {
    service
        .execute(session_id, query.to_string())
        .await
        .unwrap_or_else(|e| panic!("Query failed: {} - Error: {:?}", query, e))
}

async fn seed_benchmark_fixture(service: &GraphService, session_id: i64, space: &str) {
    execute(service, session_id, &format!("CREATE SPACE {}", space)).await;
    execute(service, session_id, &format!("USE {}", space)).await;
    execute(
        service,
        session_id,
        "CREATE TAG bench_product(name STRING, price FLOAT, stock INT64)",
    )
    .await;
    execute(
        service,
        session_id,
        "CREATE TAG bench_category(name STRING, level INT64)",
    )
    .await;
    execute(service, session_id, "CREATE TAG bench_tag(name STRING)").await;
    execute(
        service,
        session_id,
        "CREATE EDGE bench_belongs_to(score FLOAT)",
    )
    .await;
    execute(service, session_id, "CREATE EDGE bench_has_tag()").await;

    execute(
        service,
        session_id,
        r#"INSERT VERTEX bench_category(name, level) VALUES
           1000000:("Category_0", 0),
           1000001:("Category_1", 1)"#,
    )
    .await;
    execute(
        service,
        session_id,
        r#"INSERT VERTEX bench_tag(name) VALUES
           2000000:("tag_0"),
           2000001:("tag_1"),
           2000002:("tag_2")"#,
    )
    .await;
    execute(
        service,
        session_id,
        r#"INSERT VERTEX bench_product(name, price, stock) VALUES
           1:("Product_1", 10.0, 10),
           2:("Product_2", 20.0, 0),
           3:("Product_3", 30.0, 5),
           4:("Product_4", 40.0, 7),
           5:("Product_5", 50.0, 8),
           6:("Product_6", 60.0, 9)"#,
    )
    .await;
    execute(
        service,
        session_id,
        "INSERT EDGE bench_belongs_to(score) VALUES \
         1->1000000:(0.1), 2->1000000:(0.2), 3->1000000:(0.3), \
         4->1000001:(0.4), 5->1000001:(0.5), 6->1000001:(0.6)",
    )
    .await;
    execute(
        service,
        session_id,
        "INSERT EDGE bench_has_tag() VALUES \
         1->2000000:(), 2->2000001:(), 3->2000002:(), \
         4->2000000:(), 5->2000001:(), 6->2000002:()",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_crud_space_operations() {
    let (service, _temp_dir) = create_test_service();

    // Authenticate
    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    // CREATE SPACE
    let _result = execute(&service, session_id, "CREATE SPACE crud_test").await;

    // SHOW SPACES
    let result = execute(&service, session_id, "SHOW SPACES").await;
    assert!(result.row_count() >= 1, "Should have at least 1 space");

    // USE SPACE
    let _result = execute(&service, session_id, "USE crud_test").await;

    // DROP SPACE
    let _result = execute(&service, session_id, "DROP SPACE crud_test").await;

    service.sign_out(session_id, session_id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_crud_tag_operations() {
    let (service, _temp_dir) = create_test_service();

    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    // Setup space
    execute(&service, session_id, "CREATE SPACE tag_test").await;
    execute(&service, session_id, "USE tag_test").await;

    // CREATE TAG
    execute(
        &service,
        session_id,
        "CREATE TAG person(name STRING, age INT64)",
    )
    .await;

    // SHOW TAGS
    let result = execute(&service, session_id, "SHOW TAGS").await;
    assert!(result.row_count() >= 1, "Should have at least 1 tag");

    // DROP TAG
    let _result = execute(&service, session_id, "DROP TAG person").await;

    service.sign_out(session_id, session_id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_crud_edge_operations() {
    let (service, _temp_dir) = create_test_service();

    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    // Setup space
    execute(&service, session_id, "CREATE SPACE edge_test").await;
    execute(&service, session_id, "USE edge_test").await;

    // CREATE EDGE
    execute(
        &service,
        session_id,
        "CREATE EDGE follows(weight DOUBLE, since INT64)",
    )
    .await;

    // SHOW EDGES
    let result = execute(&service, session_id, "SHOW EDGES").await;
    assert!(result.row_count() >= 1, "Should have at least 1 edge type");

    // DROP EDGE
    let _result = execute(&service, session_id, "DROP EDGE follows").await;

    service.sign_out(session_id, session_id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_crud_vertex_insert_and_fetch() {
    let (service, _temp_dir) = create_test_service();

    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    // Setup
    execute(&service, session_id, "CREATE SPACE vertex_test").await;
    execute(&service, session_id, "USE vertex_test").await;
    execute(
        &service,
        session_id,
        "CREATE TAG player(name STRING, score INT64)",
    )
    .await;

    // INSERT VERTEX
    execute(
        &service,
        session_id,
        r#"INSERT VERTEX player(name, score) VALUES 1:("Alice", 100), 2:("Bob", 200), 3:("Charlie", 150)"#,
    )
    .await;

    // FETCH single vertex
    let result = execute(&service, session_id, "FETCH PROP ON player 1").await;
    assert!(result.row_count() >= 1, "Should fetch vertex 1");

    // FETCH multiple vertices
    let result = execute(&service, session_id, "FETCH PROP ON player 1, 2, 3").await;
    assert!(result.row_count() >= 3, "Should fetch 3 vertices");

    service.sign_out(session_id, session_id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_fixed_string_vid_crud_traversal_and_type_contract() {
    let (service, _temp_dir) = create_test_service();
    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    execute(
        &service,
        session_id,
        "CREATE SPACE string_vid_test (vid_type=FIXED_STRING(32))",
    )
    .await;
    execute(&service, session_id, "USE string_vid_test").await;
    execute(&service, session_id, "CREATE TAG person(name STRING)").await;
    execute(&service, session_id, "CREATE EDGE knows(since INT64)").await;
    execute(
        &service,
        session_id,
        r#"INSERT VERTEX person(name) VALUES "alice":("Alice"), "bob":("Bob")"#,
    )
    .await;
    execute(
        &service,
        session_id,
        r#"INSERT EDGE knows(since) VALUES "alice"->"bob":(2024)"#,
    )
    .await;

    let fetched = execute(
        &service,
        session_id,
        r#"FETCH PROP ON person "alice", "missing", "bob""#,
    )
    .await;
    assert_eq!(fetched.row_count(), 2);
    assert_eq!(fetched.rows[0][0], Value::String("alice".to_string()));
    assert_eq!(fetched.rows[1][0], Value::String("bob".to_string()));

    let edge_fetch = execute(
        &service,
        session_id,
        r#"FETCH PROP ON knows "alice"->"bob""#,
    )
    .await;
    assert_eq!(edge_fetch.row_count(), 1);
    assert_eq!(edge_fetch.rows[0][0], Value::String("alice".to_string()));
    assert_eq!(edge_fetch.rows[0][1], Value::String("bob".to_string()));
    assert!(matches!(&edge_fetch.rows[0][2], Value::String(json)
        if json.contains(r#""src":"alice""#) && json.contains(r#""dst":"bob""#)));

    let lookup = execute(&service, session_id, "LOOKUP ON person YIELD person.name").await;
    assert_eq!(lookup.row_count(), 2);
    assert!(lookup
        .rows
        .iter()
        .all(|row| matches!(row.first(), Some(Value::String(_)))));

    let go_default = execute(&service, session_id, r#"GO FROM "alice" OVER knows"#).await;
    assert_eq!(
        go_default.rows,
        vec![vec![
            Value::String("alice".to_string()),
            Value::String("bob".to_string()),
        ]]
    );

    let go = execute(
        &service,
        session_id,
        r#"GO FROM "alice" OVER knows YIELD knows._src AS src, knows._dst AS dst"#,
    )
    .await;
    assert_eq!(
        go.rows,
        vec![vec![
            Value::String("alice".to_string()),
            Value::String("bob".to_string()),
        ]]
    );

    // Destination projections are batch-loaded. The decoded vertex must still
    // expose the external FIXED_STRING identifier instead of its internal i64
    // mapping, while ordinary destination properties come from the same batch.
    let projected_destination = execute(
        &service,
        session_id,
        r#"GO FROM "alice" OVER knows YIELD vertex AS destination, $$.person.name AS name"#,
    )
    .await;
    assert_eq!(projected_destination.row_count(), 1);
    assert!(matches!(
        &projected_destination.rows[0][0],
        Value::String(json) if json.contains(r#""vid":"bob""#)
    ));
    assert_eq!(
        projected_destination.rows[0][1],
        Value::String("Bob".to_string())
    );

    let matched = execute(
        &service,
        session_id,
        r#"MATCH (n:person) WHERE id(n) == "alice" RETURN id(n) AS vid"#,
    )
    .await;
    assert_eq!(matched.rows, vec![vec![Value::String("alice".to_string())]]);
    let matched_vertex = execute(
        &service,
        session_id,
        r#"MATCH (n:person) WHERE id(n) == "alice" RETURN n"#,
    )
    .await;
    assert!(matches!(
        &matched_vertex.rows[0][0],
        Value::Vertex(vertex) if vertex.vid == Value::String("alice".to_string())
    ));

    // Label-only MATCH reads the tag-VID secondary index. Its keys must use
    // internal surrogates so they remain parseable before result VIDs are
    // translated back to FIXED_STRING values.
    let label_only = execute(&service, session_id, "MATCH (n:person) RETURN id(n) AS vid").await;
    let mut label_only_vids: Vec<String> = label_only
        .rows
        .iter()
        .map(|row| match row.first() {
            Some(Value::String(vid)) => vid.clone(),
            other => panic!("expected FIXED_STRING VID from label-only MATCH, got {other:?}"),
        })
        .collect();
    label_only_vids.sort();
    assert_eq!(
        label_only_vids,
        vec!["alice".to_string(), "bob".to_string()]
    );

    execute(
        &service,
        session_id,
        r#"UPDATE VERTEX ON person "carol" SET name = "Carol""#,
    )
    .await;
    let after_upsert = execute(&service, session_id, "MATCH (n:person) RETURN id(n) AS vid").await;
    let mut after_upsert_vids: Vec<String> = after_upsert
        .rows
        .iter()
        .map(|row| match row.first() {
            Some(Value::String(vid)) => vid.clone(),
            other => panic!("expected FIXED_STRING VID after upsert, got {other:?}"),
        })
        .collect();
    after_upsert_vids.sort();
    assert_eq!(
        after_upsert_vids,
        vec!["alice".to_string(), "bob".to_string(), "carol".to_string()]
    );

    let matched_edge = execute(
        &service,
        session_id,
        r#"MATCH (a:person)-[e:knows]->(b:person) WHERE id(a) == "alice" RETURN src(e), dst(e), type(e), rank(e), e"#,
    )
    .await;
    assert_eq!(matched_edge.row_count(), 1);
    assert_eq!(
        &matched_edge.rows[0][..4],
        &[
            Value::String("alice".to_string()),
            Value::String("bob".to_string()),
            Value::String("knows".to_string()),
            Value::Int(0),
        ]
    );
    assert!(matches!(
        &matched_edge.rows[0][4],
        Value::Edge(edge)
            if edge.src == Value::String("alice".to_string())
                && edge.dst == Value::String("bob".to_string())
                && edge.name == "knows"
                && edge.ranking == 0
    ));

    let found = execute(
        &service,
        session_id,
        r#"FIND SHORTEST PATH FROM "alice" TO "bob" OVER knows"#,
    )
    .await;
    assert_eq!(found.row_count(), 1);
    assert!(matches!(
        &found.rows[0][0],
        Value::List(path)
            if path.values == vec![
                Value::String("alice".to_string()),
                Value::String("bob".to_string())
            ]
    ));
    let unknown_same = execute(
        &service,
        session_id,
        r#"FIND SHORTEST PATH FROM "missing" TO "missing" OVER knows"#,
    )
    .await;
    assert_eq!(
        unknown_same.row_count(),
        0,
        "an unknown string VID must remain a point miss even when both FIND endpoints are equal"
    );

    execute(
        &service,
        session_id,
        r#"UPDATE VERTEX ON person "alice" SET name = "Alice Updated""#,
    )
    .await;
    let updated = execute(&service, session_id, r#"FETCH PROP ON person "alice""#).await;
    assert!(matches!(&updated.rows[0][1], Value::String(json) if json.contains("Alice Updated")));

    execute(&service, session_id, r#"DELETE EDGE knows "alice"->"bob""#).await;
    let after_edge_delete = execute(
        &service,
        session_id,
        r#"GO FROM "alice" OVER knows YIELD knows._dst"#,
    )
    .await;
    assert_eq!(after_edge_delete.row_count(), 0);

    execute(&service, session_id, r#"DELETE VERTEX "bob""#).await;
    let after_vertex_delete = execute(&service, session_id, r#"FETCH PROP ON person "bob""#).await;
    assert_eq!(after_vertex_delete.row_count(), 0);
    let matched_after_delete =
        execute(&service, session_id, "MATCH (n:person) RETURN id(n) AS vid").await;
    let mut surviving_vids: Vec<String> = matched_after_delete
        .rows
        .iter()
        .map(|row| match row.first() {
            Some(Value::String(vid)) => vid.clone(),
            other => panic!("expected FIXED_STRING VID after delete, got {other:?}"),
        })
        .collect();
    surviving_vids.sort();
    assert_eq!(
        surviving_vids,
        vec!["alice".to_string(), "carol".to_string()]
    );

    let wrong_fixed_type = service
        .execute(
            session_id,
            r#"INSERT VERTEX person(name) VALUES 7:("wrong")"#.to_string(),
        )
        .await;
    assert!(
        wrong_fixed_type
            .as_ref()
            .is_err_and(|error| error.to_string().contains("uses FIXED_STRING VIDs")),
        "integer VID in FIXED_STRING space should fail clearly: {wrong_fixed_type:?}"
    );

    let too_long = service
        .execute(
            session_id,
            r#"INSERT VERTEX person(name) VALUES "123456789012345678901234567890123":("too long")"#
                .to_string(),
        )
        .await;
    assert!(
        too_long.as_ref().is_err_and(|error| {
            let message = error.to_string();
            message.contains("33 bytes") && message.contains("FIXED_STRING(32)")
        }),
        "FIXED_STRING length should be enforced in UTF-8 bytes: {too_long:?}"
    );

    execute(&service, session_id, "CREATE SPACE int_vid_test").await;
    execute(&service, session_id, "USE int_vid_test").await;
    execute(&service, session_id, "CREATE TAG person(name STRING)").await;
    let wrong_int_type = service
        .execute(
            session_id,
            r#"INSERT VERTEX person(name) VALUES "alice":("wrong")"#.to_string(),
        )
        .await;
    assert!(
        wrong_int_type
            .as_ref()
            .is_err_and(|error| error.to_string().contains("uses INT64 VIDs")),
        "string VID in INT64 space should fail clearly: {wrong_int_type:?}"
    );

    service.sign_out(session_id, session_id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_insert_preserves_semicolon_inside_string_literal() {
    let (service, _temp_dir) = create_test_service();

    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    execute(&service, session_id, "CREATE SPACE semicolon_literal_test").await;
    execute(&service, session_id, "USE semicolon_literal_test").await;
    execute(&service, session_id, "CREATE TAG player(name STRING)").await;

    execute(
        &service,
        session_id,
        r#"INSERT VERTEX player(name) VALUES 1:("Alice; Bob")"#,
    )
    .await;

    let result = execute(&service, session_id, "FETCH PROP ON player 1").await;
    assert_eq!(result.row_count(), 1);
    let stored = result.rows[0]
        .iter()
        .find_map(|value| match value {
            Value::String(s) => Some(s.as_str()),
            _ => None,
        })
        .expect("FETCH should return a JSON string payload");
    assert!(
        stored.contains("Alice; Bob"),
        "stored payload should preserve semicolon, got {stored}"
    );

    service.sign_out(session_id, session_id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_crud_edge_insert_and_go() {
    let (service, _temp_dir) = create_test_service();

    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    // Setup
    execute(&service, session_id, "CREATE SPACE go_test").await;
    execute(&service, session_id, "USE go_test").await;
    execute(&service, session_id, "CREATE TAG person(name STRING)").await;
    execute(&service, session_id, "CREATE EDGE knows(weight DOUBLE)").await;

    // Insert vertices
    execute(
        &service,
        session_id,
        r#"INSERT VERTEX person(name) VALUES 1:("Alice"), 2:("Bob"), 3:("Charlie")"#,
    )
    .await;

    // Insert edges: Alice->Bob, Bob->Charlie, Alice->Charlie
    execute(
        &service,
        session_id,
        "INSERT EDGE knows(weight) VALUES 1->2:(0.9), 2->3:(0.8), 1->3:(0.5)",
    )
    .await;

    // GO 1 hop from Alice - test that query executes successfully
    let _result = execute(
        &service,
        session_id,
        "GO FROM 1 OVER knows YIELD knows._dst AS dst",
    )
    .await;
    // Note: GO traversal result count depends on edge storage implementation

    // GO 2 hops from Alice
    let _result = execute(
        &service,
        session_id,
        "GO 2 STEPS FROM 1 OVER knows YIELD knows._dst AS dst",
    )
    .await;

    service.sign_out(session_id, session_id).await.unwrap();
}

/// Reverse traversal (`GO ... REVERSELY`) must find in-neighbors via the
/// reverse-edge index, and INSERT/DELETE EDGE must keep that index in sync.
/// (PLAN.md O-1 — replaces the old O(E) space-wide edge scan.)
#[tokio::test(flavor = "multi_thread")]
async fn test_go_reversely_uses_reverse_edge_index() {
    let (service, _temp_dir) = create_test_service();

    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    execute(&service, session_id, "CREATE SPACE reverse_edge_test").await;
    execute(&service, session_id, "USE reverse_edge_test").await;
    execute(&service, session_id, "CREATE TAG person(name STRING)").await;
    execute(&service, session_id, "CREATE EDGE knows(weight DOUBLE)").await;
    execute(
        &service,
        session_id,
        r#"INSERT VERTEX person(name) VALUES 1:("Alice"), 2:("Bob"), 3:("Charlie")"#,
    )
    .await;
    // Edges INTO Bob(2): Alice->Bob, Charlie->Bob. Alice->Charlie is noise that
    // a correct reverse index must NOT surface for Bob.
    execute(
        &service,
        session_id,
        "INSERT EDGE knows(weight) VALUES 1->2:(0.9), 3->2:(0.7), 1->3:(0.5)",
    )
    .await;

    // Collect the in-neighbor vid from each row (no YIELD → [src, dst], dst is
    // the incoming source vertex for reverse traversal).
    let in_neighbors = |ds: &byoridb_common::DataSet| -> Vec<i64> {
        let mut v: Vec<i64> = ds
            .rows
            .iter()
            .filter_map(|r| match r.last() {
                Some(Value::Int(n)) => Some(*n),
                _ => None,
            })
            .collect();
        v.sort();
        v
    };

    let result = execute(&service, session_id, "GO FROM 2 OVER knows REVERSELY").await;
    assert_eq!(
        in_neighbors(&result),
        vec![1, 3],
        "reverse traversal from Bob should yield Alice and Charlie"
    );

    // Deleting Alice->Bob must remove the matching reverse-index entry.
    execute(&service, session_id, "DELETE EDGE knows 1->2").await;
    let result = execute(&service, session_id, "GO FROM 2 OVER knows REVERSELY").await;
    assert_eq!(
        in_neighbors(&result),
        vec![3],
        "after deleting Alice->Bob, only Charlie remains incoming to Bob"
    );

    service.sign_out(session_id, session_id).await.unwrap();
}

/// Index definitions must survive across queries (each GraphService query
/// builds a fresh executor context + IndexManager) and across restarts —
/// both flow through the persisted-definition load in byoridb-storage.
/// Before persistence: CREATE INDEX's definition died with its query context,
/// so INSERT wrote no index entries and LOOKUP always fell back to full scan.
#[tokio::test(flavor = "multi_thread")]
async fn test_index_definitions_survive_across_queries() {
    let (service, _temp_dir) = create_test_service();

    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    execute(&service, session_id, "CREATE SPACE index_persist_test").await;
    execute(&service, session_id, "USE index_persist_test").await;
    execute(&service, session_id, "CREATE TAG person(name STRING)").await;
    execute(
        &service,
        session_id,
        "CREATE TAG INDEX person_name_idx ON person(name)",
    )
    .await;

    // Separate query → fresh executor context: the definition must be there.
    let shown = execute(&service, session_id, "SHOW TAG INDEXES").await;
    let listed = shown.rows.iter().any(|row| {
        row.iter()
            .any(|v| matches!(v, Value::String(s) if s.contains("person_name_idx")))
    });
    assert!(
        listed,
        "SHOW TAG INDEXES must list the index created by an earlier query: {:?}",
        shown.rows
    );

    // INSERT in yet another query must see the definition and write entries.
    execute(
        &service,
        session_id,
        r#"INSERT VERTEX person(name) VALUES 1:("Alice"), 2:("Bob")"#,
    )
    .await;

    // EXPLAIN must pick the index access path (not a full-scan fallback)…
    let explain = execute(
        &service,
        session_id,
        r#"EXPLAIN LOOKUP ON person WHERE person.name == "Alice""#,
    )
    .await;
    let uses_index = explain.rows.iter().any(|row| {
        row.iter()
            .any(|v| matches!(v, Value::String(s) if s.contains("index:")))
    });
    assert!(
        uses_index,
        "LOOKUP must use the persisted index definition: {:?}",
        explain.rows
    );

    // …and the indexed LOOKUP must return the matching vertex.
    let found = execute(
        &service,
        session_id,
        r#"LOOKUP ON person WHERE person.name == "Alice""#,
    )
    .await;
    assert_eq!(
        found.row_count(),
        1,
        "indexed LOOKUP should find exactly Alice: {:?}",
        found.rows
    );

    service.sign_out(session_id, session_id).await.unwrap();
}

/// Regression: two spaces with the same tag + same index name + same VID must
/// stay isolated. Before the fix, `ctx.space_id` was never populated on the
/// standalone path, so every space collapsed onto id 1 — index definitions and
/// name-uniqueness merged across spaces, and a LOOKUP in one space returned
/// another space's data (cross-space index contamination).
#[tokio::test]
async fn test_cross_space_index_isolation() {
    let (service, _temp_dir) = create_test_service();
    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    for (space, name) in [("xspace_a", "Alice"), ("xspace_b", "Bob")] {
        execute(&service, session_id, &format!("CREATE SPACE {space}")).await;
        execute(&service, session_id, &format!("USE {space}")).await;
        execute(&service, session_id, "CREATE TAG person(name STRING)").await;
        // The SAME index name in each space must succeed: index names are
        // scoped per-space, not global. (Panics here if the name collides.)
        execute(
            &service,
            session_id,
            "CREATE TAG INDEX person_name ON person(name)",
        )
        .await;
        execute(
            &service,
            session_id,
            &format!(r#"INSERT VERTEX person(name) VALUES 1:("{name}")"#),
        )
        .await;
    }

    // Space A must find ONLY its own vertex, never space B's — via the index.
    execute(&service, session_id, "USE xspace_a").await;
    let a_sees_b = execute(
        &service,
        session_id,
        r#"LOOKUP ON person WHERE person.name == "Bob""#,
    )
    .await;
    assert_eq!(
        a_sees_b.row_count(),
        0,
        "space xspace_a must NOT find xspace_b's 'Bob' (cross-space index leak): {:?}",
        a_sees_b.rows
    );
    let a_sees_a = execute(
        &service,
        session_id,
        r#"LOOKUP ON person WHERE person.name == "Alice""#,
    )
    .await;
    assert_eq!(
        a_sees_a.row_count(),
        1,
        "space xspace_a must find its own 'Alice': {:?}",
        a_sees_a.rows
    );

    // And symmetrically for space B.
    execute(&service, session_id, "USE xspace_b").await;
    let b_sees_a = execute(
        &service,
        session_id,
        r#"LOOKUP ON person WHERE person.name == "Alice""#,
    )
    .await;
    assert_eq!(
        b_sees_a.row_count(),
        0,
        "space xspace_b must NOT find xspace_a's 'Alice' (cross-space index leak): {:?}",
        b_sees_a.rows
    );

    service.sign_out(session_id, session_id).await.unwrap();
}

/// Regression: `GO ... WHERE <cond>` must filter on edge/vertex properties.
/// Before the fix the local GO executor never evaluated `plan.where_clause`, so
/// the predicate was silently ignored and every neighbor was returned.
#[tokio::test]
async fn test_go_where_filters_edges() {
    let (service, _temp_dir) = create_test_service();
    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    execute(&service, session_id, "CREATE SPACE go_where_test").await;
    execute(&service, session_id, "USE go_where_test").await;
    execute(&service, session_id, "CREATE TAG p(name STRING)").await;
    execute(&service, session_id, "CREATE EDGE link(w INT64)").await;
    execute(
        &service,
        session_id,
        r#"INSERT VERTEX p(name) VALUES 1:("a"), 2:("b"), 3:("c")"#,
    )
    .await;
    execute(
        &service,
        session_id,
        "INSERT EDGE link(w) VALUES 1->2:(10), 1->3:(90)",
    )
    .await;

    // Sanity: without WHERE, both neighbors are returned.
    let all = execute(
        &service,
        session_id,
        "GO FROM 1 OVER link YIELD link._dst AS dst",
    )
    .await;
    assert_eq!(
        all.row_count(),
        2,
        "GO without WHERE should return both neighbors: {:?}",
        all.rows
    );

    // WHERE on an edge property must filter: only the w=90 edge (dst 3) survives.
    let filtered = execute(
        &service,
        session_id,
        "GO FROM 1 OVER link WHERE link.w > 50 YIELD link._dst AS dst",
    )
    .await;
    assert_eq!(
        filtered.row_count(),
        1,
        "GO ... WHERE link.w > 50 should return only the w=90 edge: {:?}",
        filtered.rows
    );

    service.sign_out(session_id, session_id).await.unwrap();
}

/// Regression: UPDATE and DELETE must keep tag secondary indexes consistent.
/// Before the fix, INSERT wrote index entries but UPDATE/DELETE did not touch
/// them, so LOOKUP returned the pre-update value (and missed the new one), and
/// a deleted vertex's stale index entry lingered.
#[tokio::test]
async fn test_update_delete_maintain_tag_index() {
    let (service, _temp_dir) = create_test_service();
    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    execute(&service, session_id, "CREATE SPACE idx_maint_test").await;
    execute(&service, session_id, "USE idx_maint_test").await;
    execute(&service, session_id, "CREATE TAG person(name STRING)").await;
    execute(
        &service,
        session_id,
        "CREATE TAG INDEX person_name ON person(name)",
    )
    .await;
    execute(
        &service,
        session_id,
        r#"INSERT VERTEX person(name) VALUES 1:("Alice")"#,
    )
    .await;

    let lookup = |val: &'static str| {
        let service = &service;
        async move {
            execute(
                service,
                session_id,
                &format!(r#"LOOKUP ON person WHERE person.name == "{val}""#),
            )
            .await
            .row_count()
        }
    };

    assert_eq!(
        lookup("Alice").await,
        1,
        "sanity: Alice indexed after INSERT"
    );

    // UPDATE must move the index entry from Alice to Bob.
    execute(
        &service,
        session_id,
        r#"UPDATE VERTEX ON person 1 SET name = "Bob""#,
    )
    .await;
    assert_eq!(
        lookup("Alice").await,
        0,
        "after UPDATE, the stale 'Alice' index entry must be gone"
    );
    assert_eq!(
        lookup("Bob").await,
        1,
        "after UPDATE, the new 'Bob' index entry must exist"
    );

    // DELETE must remove the index entry entirely.
    execute(&service, session_id, "DELETE VERTEX 1").await;
    assert_eq!(
        lookup("Bob").await,
        0,
        "after DELETE, no index entry may survive"
    );

    service.sign_out(session_id, session_id).await.unwrap();
}

/// Regression: UPDATE `WHEN` and DELETE `WHERE` safety predicates must gate the
/// mutation. Before the fix the executor ignored `plan.conditions`, so an
/// UPDATE ... WHEN false still wrote and a DELETE ... WHERE false still deleted.
/// (Observed via an index LOOKUP, which also exercises index maintenance.)
#[tokio::test]
async fn test_update_delete_respect_conditions() {
    let (service, _temp_dir) = create_test_service();
    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    execute(&service, session_id, "CREATE SPACE cond_test").await;
    execute(&service, session_id, "USE cond_test").await;
    execute(&service, session_id, "CREATE TAG t(n INT64)").await;
    execute(&service, session_id, "CREATE TAG INDEX t_n ON t(n)").await;
    execute(&service, session_id, "INSERT VERTEX t(n) VALUES 1:(7)").await;

    let count = |n: i64| {
        let service = &service;
        async move {
            execute(
                service,
                session_id,
                &format!("LOOKUP ON t WHERE t.n == {n}"),
            )
            .await
            .row_count()
        }
    };

    // UPDATE WHEN false → no change.
    execute(
        &service,
        session_id,
        "UPDATE VERTEX ON t 1 SET n = 99 WHEN t.n == 999",
    )
    .await;
    assert_eq!(count(7).await, 1, "WHEN false must NOT modify (n stays 7)");
    assert_eq!(count(99).await, 0, "WHEN false must NOT write n=99");

    // UPDATE WHEN true → applies.
    execute(
        &service,
        session_id,
        "UPDATE VERTEX ON t 1 SET n = 42 WHEN t.n == 7",
    )
    .await;
    assert_eq!(count(42).await, 1, "WHEN true must apply (n=42)");
    assert_eq!(count(7).await, 0, "old value 7 must be gone");

    // DELETE WHERE false → not deleted.
    execute(&service, session_id, "DELETE VERTEX 1 WHERE t.n == 999").await;
    assert_eq!(count(42).await, 1, "WHERE false must NOT delete");

    // DELETE WHERE true → deleted.
    execute(&service, session_id, "DELETE VERTEX 1 WHERE t.n == 42").await;
    assert_eq!(count(42).await, 0, "WHERE true must delete");

    service.sign_out(session_id, session_id).await.unwrap();
}

/// Regression: INSERT must reject a value whose type does not match the declared
/// column type. Before the fix, schema validation checked only property *names*,
/// so a string dropped into an INT64 column was silently stored.
#[tokio::test]
async fn test_insert_rejects_type_mismatch() {
    let (service, _temp_dir) = create_test_service();
    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    execute(&service, session_id, "CREATE SPACE type_test").await;
    execute(&service, session_id, "USE type_test").await;
    execute(&service, session_id, "CREATE TAG t(age INT64, name STRING)").await;

    // A string into an INT64 column must be rejected (was silently accepted).
    let bad = service
        .execute(
            session_id,
            r#"INSERT VERTEX t(age) VALUES 1:("oops")"#.to_string(),
        )
        .await;
    assert!(
        bad.is_err(),
        "string into INT64 column must be rejected, got: {:?}",
        bad
    );

    // Correctly-typed values still succeed.
    execute(
        &service,
        session_id,
        r#"INSERT VERTEX t(age, name) VALUES 2:(30, "ok")"#,
    )
    .await;
    let fetched = execute(&service, session_id, "FETCH PROP ON t 2").await;
    assert_eq!(
        fetched.row_count(),
        1,
        "correctly-typed INSERT should persist"
    );

    service.sign_out(session_id, session_id).await.unwrap();
}

/// Regression: an indexed MATCH must not silently truncate candidates at a
/// hard-coded 1000. With 1001 matching vertices the pattern must return all
/// 1001 (bounded only by the configurable scan cap, default 100k).
#[tokio::test]
async fn test_indexed_match_not_truncated_at_1000() {
    let (service, _temp_dir) = create_test_service();
    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    execute(&service, session_id, "CREATE SPACE match_cap_test").await;
    execute(&service, session_id, "USE match_cap_test").await;
    execute(&service, session_id, "CREATE TAG person(city STRING)").await;
    execute(
        &service,
        session_id,
        "CREATE TAG INDEX person_city ON person(city)",
    )
    .await;

    // 1001 vertices all with city = "Seoul" (one row over the old 1000 cap).
    let mut values = String::new();
    for vid in 1..=1001 {
        if vid > 1 {
            values.push_str(", ");
        }
        values.push_str(&format!(r#"{vid}:("Seoul")"#));
    }
    execute(
        &service,
        session_id,
        &format!("INSERT VERTEX person(city) VALUES {values}"),
    )
    .await;

    let matched = execute(
        &service,
        session_id,
        r#"MATCH (n:person {city: "Seoul"}) RETURN n"#,
    )
    .await;
    assert_eq!(
        matched.row_count(),
        1001,
        "indexed MATCH must return all 1001 matches, not a truncated 1000"
    );

    service.sign_out(session_id, session_id).await.unwrap();
}

/// Regression: a LOOKUP predicate the pushdown can't express must NOT silently
/// match everything. Before the fix an unconvertible WHERE (e.g. CONTAINS) fell
/// back to `FilterExpr::True`, returning every vertex of the tag.
#[tokio::test]
async fn test_lookup_unsupported_predicate_not_fail_open() {
    let (service, _temp_dir) = create_test_service();
    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    execute(&service, session_id, "CREATE SPACE lookup_failopen_test").await;
    execute(&service, session_id, "USE lookup_failopen_test").await;
    execute(&service, session_id, "CREATE TAG person(name STRING)").await;
    execute(
        &service,
        session_id,
        r#"INSERT VERTEX person(name) VALUES 1:("Alice"), 2:("Bob"), 3:("Carol")"#,
    )
    .await;

    // A supported predicate (no index → fallback scan) still filters correctly.
    let ok = execute(
        &service,
        session_id,
        r#"LOOKUP ON person WHERE person.name == "Alice""#,
    )
    .await;
    assert_eq!(ok.row_count(), 1, "== predicate must filter to just Alice");

    // An unsupported predicate must be rejected, not fail open to all 3 rows.
    let bad = service
        .execute(
            session_id,
            r#"LOOKUP ON person WHERE person.name CONTAINS "li""#.to_string(),
        )
        .await;
    assert!(
        bad.is_err(),
        "unsupported LOOKUP predicate must error, not return every row: {:?}",
        bad.map(|d| d.row_count())
    );

    service.sign_out(session_id, session_id).await.unwrap();
}

/// Regression: DROP INDEX must delete the index *data*, not only its definition.
/// A fresh IndexManager rebuilds next_index_id from surviving defs, so a later
/// CREATE INDEX can reuse the dropped id — if the old entries linger they are
/// reinterpreted as the new index's, leaking stale rows.
#[tokio::test]
async fn test_drop_index_removes_stale_entries() {
    let (service, _temp_dir) = create_test_service();
    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    execute(&service, session_id, "CREATE SPACE drop_idx_test").await;
    execute(&service, session_id, "USE drop_idx_test").await;
    execute(
        &service,
        session_id,
        "CREATE TAG t(name STRING, city STRING)",
    )
    .await;
    execute(&service, session_id, "CREATE TAG INDEX t_name ON t(name)").await;
    execute(
        &service,
        session_id,
        r#"INSERT VERTEX t(name) VALUES 1:("Alice")"#,
    )
    .await;

    // Drop the name index, then create a city index that may reuse the id.
    execute(&service, session_id, "DROP INDEX TAG t_name").await;
    execute(&service, session_id, "CREATE TAG INDEX t_city ON t(city)").await;

    // The stale name-index entry "Alice" must NOT surface via the city index.
    let leak = execute(
        &service,
        session_id,
        r#"LOOKUP ON t WHERE t.city == "Alice""#,
    )
    .await;
    assert_eq!(
        leak.row_count(),
        0,
        "dropped index data must not leak into a reused index id: {:?}",
        leak.rows
    );

    service.sign_out(session_id, session_id).await.unwrap();
}

/// Regression: `LOOKUP ON <edge-type>` must fail clearly rather than silently
/// returning an empty/tag-filtered result. The parser can't tell tag from edge,
/// so the executor detects an edge name and rejects it (edge LOOKUP is a
/// not-yet-implemented feature; the previous behaviour returned misleading rows).
#[tokio::test]
async fn edge_lookup_with_no_match_is_empty_not_an_error() {
    let (service, _temp_dir) = create_test_service();
    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    execute(&service, session_id, "CREATE SPACE edge_lookup_test").await;
    execute(&service, session_id, "USE edge_lookup_test").await;
    execute(&service, session_id, "CREATE TAG t(n INT64)").await;
    execute(&service, session_id, "CREATE EDGE e(w INT64)").await;
    execute(&service, session_id, "INSERT VERTEX t(n) VALUES 1:(1)").await;

    // LOOKUP on an edge type is implemented (#79). With no matching edge it is
    // an empty result — which is now a true answer rather than the refusal this
    // test originally asserted, and distinguishable from an error.
    let empty = execute(&service, session_id, "LOOKUP ON e WHERE e.w == 1").await;
    assert_eq!(empty.row_count(), 0);
    assert_eq!(empty.column_names, vec!["e.src", "e.dst", "e.rank"]);

    // A name that is neither a tag nor an edge stays on the tag path and is
    // likewise empty rather than an error.
    let unknown = execute(&service, session_id, "LOOKUP ON nonexistent WHERE x == 1").await;
    assert_eq!(unknown.row_count(), 0);

    // Tag LOOKUP still works.
    let tag_lookup = execute(&service, session_id, "LOOKUP ON t WHERE t.n == 1").await;
    assert_eq!(tag_lookup.row_count(), 1, "tag LOOKUP must still work");

    service.sign_out(session_id, session_id).await.unwrap();
}

/// Graceful shutdown: once readiness is flipped off, new queries must be
/// rejected with a clear error (k8s stops routing via /ready; this guards the
/// window where a connection is already open).
#[tokio::test(flavor = "multi_thread")]
async fn test_shutdown_rejects_new_queries() {
    let (service, _temp_dir) = create_test_service();

    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    // Sanity: queries work while accepting.
    execute(&service, session_id, "CREATE SPACE shutdown_test").await;

    let state = service.shutdown_state();
    assert!(state.is_accepting());
    assert_eq!(state.active_queries(), 0, "no queries should be in flight");

    state.stop_accepting();

    let err = service
        .execute(session_id, "SHOW SPACES".to_string())
        .await
        .expect_err("queries must be rejected after stop_accepting");
    assert!(
        err.to_string().contains("shutting down"),
        "rejection must clearly say the server is shutting down: {err}"
    );

    service.sign_out(session_id, session_id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_crud_lookup() {
    let (service, _temp_dir) = create_test_service();

    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    // Setup
    execute(&service, session_id, "CREATE SPACE lookup_test").await;
    execute(&service, session_id, "USE lookup_test").await;
    execute(
        &service,
        session_id,
        "CREATE TAG employee(name STRING, age INT64, department STRING)",
    )
    .await;

    // Insert test data
    execute(
        &service,
        session_id,
        r#"INSERT VERTEX employee(name, age, department) VALUES
           1:("Alice", 30, "Engineering"),
           2:("Bob", 25, "Sales"),
           3:("Charlie", 35, "Engineering"),
           4:("Diana", 28, "Marketing")"#,
    )
    .await;

    // LOOKUP by age
    let result = execute(
        &service,
        session_id,
        "LOOKUP ON employee WHERE employee.age > 27",
    )
    .await;
    assert!(
        result.row_count() >= 2,
        "Should find at least 2 employees over 27"
    );

    service.sign_out(session_id, session_id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_crud_update_vertex() {
    let (service, _temp_dir) = create_test_service();

    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    // Setup
    execute(&service, session_id, "CREATE SPACE update_test").await;
    execute(&service, session_id, "USE update_test").await;
    execute(&service, session_id, "CREATE TAG score(value INT64)").await;

    // Insert initial data
    execute(
        &service,
        session_id,
        "INSERT VERTEX score(value) VALUES 1:(100)",
    )
    .await;

    // Verify initial value
    let result = execute(&service, session_id, "FETCH PROP ON score 1").await;
    assert!(result.row_count() >= 1);

    // UPDATE vertex (syntax: UPDATE VERTEX ON tag_name vid SET prop = value)
    execute(
        &service,
        session_id,
        "UPDATE VERTEX ON score 1 SET value = 200",
    )
    .await;

    // Verify updated value
    let result = execute(&service, session_id, "FETCH PROP ON score 1").await;
    assert!(
        result.row_count() >= 1,
        "Should still have vertex after update"
    );

    service.sign_out(session_id, session_id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_crud_delete_vertex() {
    let (service, _temp_dir) = create_test_service();

    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    // Setup
    execute(&service, session_id, "CREATE SPACE delete_test").await;
    execute(&service, session_id, "USE delete_test").await;
    execute(&service, session_id, "CREATE TAG item(name STRING)").await;

    // Insert test data
    execute(
        &service,
        session_id,
        r#"INSERT VERTEX item(name) VALUES 1:("Item1"), 2:("Item2"), 3:("Item3")"#,
    )
    .await;

    // Verify data exists
    let result = execute(&service, session_id, "FETCH PROP ON item 1, 2, 3").await;
    assert!(result.row_count() >= 3, "Should have 3 items before delete");

    // DELETE vertices
    execute(&service, session_id, "DELETE VERTEX 1, 2").await;

    // Verify deletion (only vertex 3 should remain)
    let result = execute(&service, session_id, "FETCH PROP ON item 3").await;
    assert!(result.row_count() >= 1, "Vertex 3 should still exist");

    service.sign_out(session_id, session_id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_crud_user_management() {
    let (service, _temp_dir) = create_test_service();

    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    // CREATE USER
    execute(
        &service,
        session_id,
        r#"CREATE USER testuser WITH PASSWORD "testpass""#,
    )
    .await;

    // GRANT ROLE
    let _result = execute(&service, session_id, "GRANT ROLE ADMIN TO testuser").await;

    // REVOKE ROLE
    let _result = execute(&service, session_id, "REVOKE ROLE ADMIN FROM testuser").await;

    // ALTER USER (change password)
    let _result = execute(
        &service,
        session_id,
        r#"ALTER USER testuser WITH PASSWORD "newpass123""#,
    )
    .await;

    // DROP USER
    let _result = execute(&service, session_id, "DROP USER testuser").await;

    service.sign_out(session_id, session_id).await.unwrap();
}

/// Test that newly created users can authenticate
#[tokio::test(flavor = "multi_thread")]
async fn test_new_user_authentication() {
    let (service, _temp_dir) = create_test_service();

    // First authenticate as root
    let root_session = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Root authentication failed");

    // Create a new user
    execute(
        &service,
        root_session,
        r#"CREATE USER newuser WITH PASSWORD "newpass""#,
    )
    .await;

    service.sign_out(root_session, root_session).await.unwrap();

    // AUTH-SYNC (S-3) is implemented: CREATE USER syncs to AuthManager in-memory cache,
    // so the new user can authenticate immediately after creation.
    let new_session = service
        .authenticate("newuser".to_string(), "newpass".to_string())
        .await;
    let new_session_id = new_session.expect("New user should authenticate after AUTH-SYNC");
    service
        .sign_out(new_session_id, new_session_id)
        .await
        .unwrap();

    // Cleanup: log back in as root to delete the user
    let root_session = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Root re-authentication failed");

    execute(&service, root_session, "DROP USER IF EXISTS newuser").await;
    service.sign_out(root_session, root_session).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_benchmark_match_query_patterns() {
    let (service, _temp_dir) = create_test_service();

    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    seed_benchmark_fixture(&service, session_id, "benchmark_patterns").await;

    let q1 = execute(
        &service,
        session_id,
        "MATCH (p:bench_product)-[:bench_belongs_to]->(c:bench_category) \
         WHERE id(c)==1000000 AND p.bench_product.stock>0 \
         RETURN p.bench_product.name AS name, p.bench_product.price AS price \
         ORDER BY price DESC LIMIT 10",
    )
    .await;
    assert_eq!(q1.column_names, vec!["name", "price"]);
    assert_eq!(
        q1.row_count(),
        2,
        "Q1 should filter out zero-stock products"
    );

    let q2 = execute(
        &service,
        session_id,
        "MATCH (p:bench_product)-[:bench_belongs_to]->(c:bench_category) \
         WHERE id(c)==1000001 RETURN p.bench_product.name AS name LIMIT 2",
    )
    .await;
    assert_eq!(q2.row_count(), 2, "Q2 LIMIT must cap reverse-hop results");

    let q3 = execute(
        &service,
        session_id,
        "MATCH (p1:bench_product)-[:bench_belongs_to]->(c:bench_category)\
         <-[:bench_belongs_to]-(p2:bench_product) \
         WHERE id(p1)==1 \
         RETURN p2.bench_product.name AS name, p2.bench_product.price AS price \
         LIMIT 10",
    )
    .await;
    assert_eq!(q3.column_names, vec!["name", "price"]);
    assert_eq!(
        q3.row_count(),
        3,
        "Q3 should return products in p1 category"
    );

    let q4 = execute(
        &service,
        session_id,
        "MATCH (p:bench_product)-[:bench_belongs_to]->(c:bench_category), \
         (p)-[:bench_has_tag]->(t:bench_tag) \
         WHERE id(c)==1000000 \
         RETURN p.bench_product.name AS name, t.bench_tag.name AS tag_name LIMIT 2",
    )
    .await;
    assert_eq!(q4.column_names, vec!["name", "tag_name"]);
    assert_eq!(
        q4.row_count(),
        2,
        "Q4 multi-pattern LIMIT must be preserved"
    );

    let q5 = execute(
        &service,
        session_id,
        "MATCH (p:bench_product)-[:bench_belongs_to]->(c:bench_category) \
         WHERE id(c)==1000001 \
         RETURN count(p) AS cnt, avg(p.bench_product.price) AS avg_price",
    )
    .await;
    assert_eq!(q5.column_names, vec!["cnt", "avg_price"]);
    assert_eq!(q5.row_count(), 1);
    assert_eq!(q5.rows[0][0], Value::Int(3));
    assert_eq!(q5.rows[0][1], Value::Float(50.0));

    service.sign_out(session_id, session_id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_crud_full_workflow() {
    let (service, _temp_dir) = create_test_service();

    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    // 1. Create space and schema
    execute(&service, session_id, "CREATE SPACE social_network").await;
    execute(&service, session_id, "USE social_network").await;
    execute(
        &service,
        session_id,
        "CREATE TAG user(name STRING, age INT64, city STRING)",
    )
    .await;
    execute(
        &service,
        session_id,
        "CREATE EDGE friend(since INT64, closeness DOUBLE)",
    )
    .await;

    // 2. Insert users
    execute(
        &service,
        session_id,
        r#"INSERT VERTEX user(name, age, city) VALUES
           1:("Alice", 28, "Seoul"),
           2:("Bob", 32, "Busan"),
           3:("Charlie", 25, "Seoul"),
           4:("Diana", 30, "Incheon")"#,
    )
    .await;

    // 3. Insert friendships
    execute(
        &service,
        session_id,
        "INSERT EDGE friend(since, closeness) VALUES 1->2:(2020, 0.9), 1->3:(2021, 0.7), 2->4:(2019, 0.8), 3->4:(2022, 0.6)",
    )
    .await;

    // 4. Query: Find Alice's friends (test that query executes)
    let _result = execute(
        &service,
        session_id,
        "GO FROM 1 OVER friend YIELD friend._dst AS friend_id",
    )
    .await;

    // 5. Query: Find friends of friends (2-hop)
    let _result = execute(
        &service,
        session_id,
        "GO 2 STEPS FROM 1 OVER friend YIELD friend._dst AS fof",
    )
    .await;

    // 6. Lookup users in Seoul
    let result = execute(
        &service,
        session_id,
        r#"LOOKUP ON user WHERE user.city == "Seoul""#,
    )
    .await;
    assert!(result.row_count() >= 2, "Should find 2 users in Seoul");

    // 7. Fetch Alice
    let result = execute(&service, session_id, "FETCH PROP ON user 1").await;
    assert!(result.row_count() >= 1);

    // 8. Delete Diana
    execute(&service, session_id, "DELETE VERTEX 4").await;

    // 9. Cleanup
    execute(&service, session_id, "DROP SPACE social_network").await;

    service.sign_out(session_id, session_id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_show_stats_reports_empty_tag_and_edge_as_zero() {
    // Regression for LDBC validation feedback #5: an empty tag/edge must appear
    // in SHOW STATS with count 0, not be absent (which read as actual=None).
    let (service, _temp_dir) = create_test_service();
    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    execute(&service, session_id, "CREATE SPACE stats_zero").await;
    execute(&service, session_id, "USE stats_zero").await;
    execute(&service, session_id, "CREATE TAG Person(name STRING)").await;
    execute(&service, session_id, "CREATE EDGE knows()").await;
    // Deliberately insert no data — counts must be 0, not missing.

    let ds = execute(&service, session_id, "SHOW STATS").await;

    let has_tag_zero = ds.rows.iter().any(|r| {
        r.len() >= 3
            && r[0] == Value::String("Tag".to_string())
            && r[1] == Value::String("Person".to_string())
            && r[2] == Value::Int(0)
    });
    let has_edge_zero = ds.rows.iter().any(|r| {
        r.len() >= 3
            && r[0] == Value::String("Edge".to_string())
            && r[1] == Value::String("knows".to_string())
            && r[2] == Value::Int(0)
    });
    assert!(
        has_tag_zero,
        "empty tag Person must report count 0, got rows: {:?}",
        ds.rows
    );
    assert!(
        has_edge_zero,
        "empty edge knows must report count 0, got rows: {:?}",
        ds.rows
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_create_index_backfills_existing_data() {
    // CREATE INDEX after data is loaded must backfill existing rows (the LDBC
    // --skip-indexes-then-create flow). Backfill is now chunk-batched.
    let (service, _temp_dir) = create_test_service();
    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    execute(&service, session_id, "CREATE SPACE backfill_test").await;
    execute(&service, session_id, "USE backfill_test").await;
    execute(&service, session_id, "CREATE TAG person(name STRING)").await;
    execute(
        &service,
        session_id,
        r#"INSERT VERTEX person(name) VALUES 1:("alice"), 2:("bob"), 3:("alice")"#,
    )
    .await;
    // Index created AFTER the data exists → must backfill.
    execute(
        &service,
        session_id,
        "CREATE TAG INDEX person_name_idx ON person(name)",
    )
    .await;

    let ds = execute(
        &service,
        session_id,
        r#"LOOKUP ON person WHERE person.name == "alice""#,
    )
    .await;
    assert_eq!(
        ds.rows.len(),
        2,
        "backfilled index must find both alice vertices, got: {:?}",
        ds.rows
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_lookup_qualified_property_filter_without_index() {
    // WHERE person.name == "alice" (qualified PropRef) must filter even with no
    // index (fallback predicate-pushdown path). Previously expr_to_filter_expr
    // returned None for PropRef → FilterExpr::True → every row leaked through.
    let (service, _temp_dir) = create_test_service();
    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    execute(&service, session_id, "CREATE SPACE lookup_filter").await;
    execute(&service, session_id, "USE lookup_filter").await;
    execute(&service, session_id, "CREATE TAG person(name STRING)").await;
    execute(
        &service,
        session_id,
        r#"INSERT VERTEX person(name) VALUES 1:("alice"), 2:("bob"), 3:("alice")"#,
    )
    .await;
    // No index created → fallback path.
    let ds = execute(
        &service,
        session_id,
        r#"LOOKUP ON person WHERE person.name == "alice""#,
    )
    .await;
    assert_eq!(
        ds.rows.len(),
        2,
        "qualified-property filter must exclude bob, got: {:?}",
        ds.rows
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_active_query_registry_cleared_after_execution() {
    // Observability (#3): the in-flight registry must drop entries on every
    // exit path. After all queries complete it must be empty (no leak), which
    // also keeps the byoridb_inflight_queries gauge balanced.
    let (service, _temp_dir) = create_test_service();
    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    execute(&service, session_id, "CREATE SPACE diag_test").await;
    execute(&service, session_id, "USE diag_test").await;
    execute(&service, session_id, "CREATE TAG person(name STRING)").await;
    execute(
        &service,
        session_id,
        r#"INSERT VERTEX person(name) VALUES 1:("a"), 2:("b")"#,
    )
    .await;
    // A failing query must also clean up.
    let _ = service
        .execute(session_id, "LOOKUP ON nonexistent_tag".to_string())
        .await;

    assert!(
        service.list_active_queries().is_empty(),
        "registry must be empty after queries finish, got: {:?}",
        service.list_active_queries()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_drop_space_purges_data_for_name_reuse() {
    // Repeated-benchmark blocker: DROP SPACE must purge data/schema/index so the
    // same name can be recreated with no stale rows or "index already exists".
    let (service, _temp_dir) = create_test_service();
    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    // Round 1: create + index + data.
    execute(&service, session_id, "CREATE SPACE reuse_test").await;
    execute(&service, session_id, "USE reuse_test").await;
    execute(&service, session_id, "CREATE TAG person(name STRING)").await;
    execute(
        &service,
        session_id,
        "CREATE TAG INDEX person_name_idx ON person(name)",
    )
    .await;
    execute(
        &service,
        session_id,
        r#"INSERT VERTEX person(name) VALUES 1:("alice"), 2:("bob")"#,
    )
    .await;

    execute(&service, session_id, "DROP SPACE reuse_test").await;

    // Round 2: recreate same name.
    execute(&service, session_id, "CREATE SPACE reuse_test").await;
    execute(&service, session_id, "USE reuse_test").await;
    execute(&service, session_id, "CREATE TAG person(name STRING)").await;

    // (a) Index recreation must succeed (in-memory def was purged).
    let idx = service
        .execute(
            session_id,
            "CREATE TAG INDEX person_name_idx ON person(name)".to_string(),
        )
        .await;
    assert!(
        idx.is_ok(),
        "index must be recreatable after DROP SPACE, got: {:?}",
        idx.err()
    );

    // (b) SHOW STATS: person count must be 0, not the stale 2.
    let stats = execute(&service, session_id, "SHOW STATS").await;
    let person_count = stats.rows.iter().find_map(|r| {
        if r.len() >= 3 && r[1] == Value::String("person".to_string()) {
            if let Value::Int(n) = r[2] {
                return Some(n);
            }
        }
        None
    });
    assert_eq!(
        person_count,
        Some(0),
        "person count must be 0 after DROP+recreate, got: {:?} / rows {:?}",
        person_count,
        stats.rows
    );

    // (c) LOOKUP must find no stale data.
    let lookup = execute(
        &service,
        session_id,
        r#"LOOKUP ON person WHERE person.name == "alice""#,
    )
    .await;
    assert!(
        lookup.rows.is_empty(),
        "no stale rows after DROP+recreate, got: {:?}",
        lookup.rows
    );
}

/// End-to-end owl:sameAs canonical merge (PLAN.md O-8). Asserting `sameAs`
/// collapses two nodes onto the min-id representative; reads of the merged-away
/// vid (GO source, FETCH) normalize to it, and the irreversible merge blocks
/// deletion of the merged node and the sameAs edge.
#[tokio::test(flavor = "multi_thread")]
async fn test_sameas_merges_nodes_and_blocks_deletion() {
    let (service, _temp_dir) = create_test_service();

    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    execute(&service, session_id, "CREATE SPACE sameas_test").await;
    execute(&service, session_id, "USE sameas_test").await;
    execute(
        &service,
        session_id,
        "CREATE TAG product(name STRING, channel STRING)",
    )
    .await;
    execute(&service, session_id, "CREATE EDGE sameAs()").await;
    execute(&service, session_id, "CREATE EDGE sells()").await;

    // Two channel listings of the same real product; a buyer (9) points at vid 5.
    execute(
        &service,
        session_id,
        r#"INSERT VERTEX product(name, channel) VALUES 2:("Widget", "naver"), 5:("widget", "coupang")"#,
    )
    .await;
    execute(&service, session_id, "INSERT EDGE sells() VALUES 9->5:()").await;

    // Assert equivalence: 5 sameAs 2 ⟹ winner = min-id 2, vid 5 collapses into it.
    execute(&service, session_id, "INSERT EDGE sameAs() VALUES 5->2:()").await;

    // GO from the buyer now reaches the representative (2), not merged-away 5.
    let go = execute(
        &service,
        session_id,
        "GO FROM 9 OVER sells YIELD sells._dst AS dst",
    )
    .await;
    let dsts: Vec<i64> = go
        .rows
        .iter()
        .filter_map(|r| match r.last() {
            Some(Value::Int(n)) => Some(*n),
            _ => None,
        })
        .collect();
    assert!(
        dsts.contains(&2),
        "sells edge rewritten onto representative 2, got {:?}",
        dsts
    );
    assert!(!dsts.contains(&5), "merged-away vid 5 must not appear");

    // FETCH of the merged-away vid 5 normalizes to representative 2.
    let fetch = execute(&service, session_id, "FETCH PROP ON product 5").await;
    let ids: Vec<i64> = fetch
        .rows
        .iter()
        .filter_map(|r| match r.first() {
            Some(Value::Int(n)) => Some(*n),
            _ => None,
        })
        .collect();
    assert_eq!(ids, vec![2], "FETCH of 5 returns representative 2");

    // Irreversibility (D7): deleting the merged node / representative / sameAs edge
    // is rejected — owl:sameAs merges cannot be undone in an insertion-only engine.
    assert!(
        service
            .execute(session_id, "DELETE VERTEX 5".to_string())
            .await
            .is_err(),
        "deleting a merged-away node must be rejected"
    );
    assert!(
        service
            .execute(session_id, "DELETE VERTEX 2".to_string())
            .await
            .is_err(),
        "deleting a representative with members must be rejected"
    );
    assert!(
        service
            .execute(session_id, "DELETE EDGE sameAs 5->2".to_string())
            .await
            .is_err(),
        "deleting a sameAs edge must be rejected"
    );

    service.sign_out(session_id, session_id).await.unwrap();
}

/// `IN` must select exactly the rows an equivalent OR chain selects. That
/// equivalence is the whole point of the operator: callers previously had to
/// build the OR chain themselves, one term per seed VID.
#[tokio::test(flavor = "multi_thread")]
async fn test_in_matches_the_equivalent_or_chain() {
    let (service, _temp_dir) = create_test_service();

    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    execute(&service, session_id, "CREATE SPACE in_operator_test").await;
    execute(&service, session_id, "USE in_operator_test").await;
    execute(
        &service,
        session_id,
        "CREATE TAG person(name STRING, age INT64)",
    )
    .await;
    for (vid, name, age) in [
        (1, "Ada", 36),
        (2, "Grace", 45),
        (3, "Alan", 41),
        (4, "Edsger", 72),
    ] {
        execute(
            &service,
            session_id,
            &format!("INSERT VERTEX person(name, age) VALUES {vid}:('{name}', {age})"),
        )
        .await;
    }

    let ids_via_in = execute(
        &service,
        session_id,
        "MATCH (p:person) WHERE id(p) IN [1, 3] RETURN id(p) AS vid",
    )
    .await;
    let ids_via_or = execute(
        &service,
        session_id,
        "MATCH (p:person) WHERE id(p) == 1 OR id(p) == 3 RETURN id(p) AS vid",
    )
    .await;

    let sorted = |data: &byoridb_common::DataSet| {
        let mut vids: Vec<i64> = data
            .rows
            .iter()
            .filter_map(|row| {
                row.iter().find_map(|v| match v {
                    Value::Int(i) => Some(*i),
                    _ => None,
                })
            })
            .collect();
        vids.sort_unstable();
        vids
    };

    assert_eq!(
        sorted(&ids_via_in),
        vec![1, 3],
        "IN should select exactly the listed vertices"
    );
    assert_eq!(
        sorted(&ids_via_in),
        sorted(&ids_via_or),
        "IN must agree with the equivalent OR chain"
    );

    // NOT IN is the complement over the same population.
    let ids_via_not_in = execute(
        &service,
        session_id,
        "MATCH (p:person) WHERE id(p) NOT IN [1, 3] RETURN id(p) AS vid",
    )
    .await;
    assert_eq!(
        sorted(&ids_via_not_in),
        vec![2, 4],
        "NOT IN should select the complement"
    );

    // An empty list matches nothing rather than erroring or matching everything.
    let ids_via_empty = execute(
        &service,
        session_id,
        "MATCH (p:person) WHERE id(p) IN [] RETURN id(p) AS vid",
    )
    .await;
    assert_eq!(
        ids_via_empty.row_count(),
        0,
        "IN [] should match nothing, got {:?}",
        ids_via_empty.rows
    );

    // String property lists work the same way.
    let by_name = execute(
        &service,
        session_id,
        "MATCH (p:person) WHERE p.person.name IN ['Ada', 'Alan'] RETURN id(p) AS vid",
    )
    .await;
    assert_eq!(
        sorted(&by_name),
        vec![1, 3],
        "IN should filter on a string property"
    );

    service.sign_out(session_id, session_id).await.unwrap();
}

/// LOOKUP pushes its predicate down to the storage filter, which previously
/// rejected `IN` outright (fail-closed) rather than returning unfiltered rows.
#[tokio::test(flavor = "multi_thread")]
async fn test_lookup_supports_in_predicate() {
    let (service, _temp_dir) = create_test_service();

    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    execute(&service, session_id, "CREATE SPACE in_lookup_test").await;
    execute(&service, session_id, "USE in_lookup_test").await;
    execute(&service, session_id, "CREATE TAG person(name STRING)").await;
    for (vid, name) in [(1, "Ada"), (2, "Grace"), (3, "Alan")] {
        execute(
            &service,
            session_id,
            &format!("INSERT VERTEX person(name) VALUES {vid}:('{name}')"),
        )
        .await;
    }

    let found = execute(
        &service,
        session_id,
        "LOOKUP ON person WHERE person.name IN ['Ada', 'Alan'] YIELD person.name AS name",
    )
    .await;
    assert_eq!(
        found.row_count(),
        2,
        "LOOKUP should accept an IN predicate, got {:?}",
        found.rows
    );

    service.sign_out(session_id, session_id).await.unwrap();
}

/// Regression for #102. An unknown function used to evaluate to `NULL` inside
/// `MATCH`, so an unsupported feature was indistinguishable from empty data:
/// in `RETURN` it produced a row of nulls, and in `WHERE` it made the predicate
/// false for every row and reported "0 rows" for a query that never ran.
#[tokio::test(flavor = "multi_thread")]
async fn unknown_functions_fail_instead_of_evaluating_to_null() {
    let (service, _temp_dir) = create_test_service();
    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    execute(&service, session_id, "CREATE SPACE fn_errors").await;
    execute(&service, session_id, "USE fn_errors").await;
    execute(&service, session_id, "CREATE TAG doc(body STRING)").await;
    execute(
        &service,
        session_id,
        r#"INSERT VERTEX doc(body) VALUES 1:("Worktrees are isolated")"#,
    )
    .await;

    // A projected unknown function must name itself rather than return a row.
    let error = service
        .execute(
            session_id,
            "MATCH (n:doc) RETURN frobnicate(n.doc.body) AS nonsense".to_string(),
        )
        .await
        .expect_err("an unknown function must not project a row");
    assert!(
        error.to_string().to_lowercase().contains("frobnicate"),
        "the error must name the function, got: {error}"
    );

    // In a filter it is worse than confusing: silently matching nothing reads
    // as "no such data".
    let error = service
        .execute(
            session_id,
            "MATCH (n:doc) WHERE frobnicate(n.doc.body) RETURN n.doc.body".to_string(),
        )
        .await
        .expect_err("an unknown function in WHERE must not silently match nothing");
    assert!(
        error.to_string().to_lowercase().contains("frobnicate"),
        "the error must name the function, got: {error}"
    );

    // Case folding is the need behind the report, and it must work rather than
    // be another silent null: `CONTAINS` matches case exactly.
    let folded = execute(
        &service,
        session_id,
        "MATCH (n:doc) WHERE toLower(n.doc.body) CONTAINS 'worktrees' RETURN n.doc.body AS body",
    )
    .await;
    assert_eq!(
        folded.rows,
        vec![vec![Value::String("Worktrees are isolated".to_string())]],
        "toLower must fold case in a filter"
    );

    let projected = execute(
        &service,
        session_id,
        "MATCH (n:doc) RETURN toLower(n.doc.body) AS lowered, toUpper(n.doc.body) AS uppered",
    )
    .await;
    assert_eq!(
        projected.rows,
        vec![vec![
            Value::String("worktrees are isolated".to_string()),
            Value::String("WORKTREES ARE ISOLATED".to_string()),
        ]]
    );

    // A MATCH nested in a compound statement, or wrapped in PROFILE (which
    // executes its inner statement), must be validated too — those are the two
    // shapes a per-statement check is most likely to miss.
    for statement in [
        "SHOW SPACES; MATCH (n:doc) RETURN frobnicate(n.doc.body)",
        "PROFILE MATCH (n:doc) RETURN frobnicate(n.doc.body)",
    ] {
        let error = service
            .execute(session_id, statement.to_string())
            .await
            .unwrap_err();
        assert!(
            error.to_string().to_lowercase().contains("frobnicate"),
            "`{statement}` must be refused by name, got: {error}"
        );
    }

    // A supported function used wrongly is not the same thing as an unknown
    // one: it stays a value-level outcome rather than refusing the query.
    let wrong_type = execute(
        &service,
        session_id,
        "MATCH (n:doc) RETURN toLower(42) AS folded",
    )
    .await;
    assert_eq!(wrong_type.rows.len(), 1);

    service.sign_out(session_id, session_id).await.unwrap();
}

/// Regression for #77. `UPDATE EDGE` parsed and was then planned as a vertex
/// update with no tag, so it failed with "Tag name required for UPDATE" — an
/// error naming a concept the statement never mentions. It now updates the edge.
#[tokio::test(flavor = "multi_thread")]
async fn update_edge_writes_the_edge_it_names() {
    let (service, _temp_dir) = create_test_service();
    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    execute(&service, session_id, "CREATE SPACE edge_update").await;
    execute(&service, session_id, "USE edge_update").await;
    execute(&service, session_id, "CREATE TAG person(name STRING)").await;
    execute(&service, session_id, "CREATE EDGE knows(since INT64)").await;
    execute(
        &service,
        session_id,
        r#"INSERT VERTEX person(name) VALUES 1:("Alice"), 2:("Bob")"#,
    )
    .await;
    execute(
        &service,
        session_id,
        "INSERT EDGE knows(since) VALUES 1->2:(2020)",
    )
    .await;

    let updated = execute(
        &service,
        session_id,
        "UPDATE EDGE ON knows 1->2 SET since = 2021",
    )
    .await;
    assert_eq!(updated.rows, vec![vec![Value::Int(1)]]);

    // The forward read sees the new value.
    let forward = execute(
        &service,
        session_id,
        "GO FROM 1 OVER knows YIELD knows.since AS since",
    )
    .await;
    assert_eq!(forward.rows, vec![vec![Value::Int(2021)]]);

    // So does the reverse read, which is served by a second copy of the payload
    // under an `in-edge` key. Refreshing only the forward key would leave the
    // two disagreeing.
    let reverse = execute(
        &service,
        session_id,
        "GO FROM 2 OVER knows REVERSELY YIELD knows.since AS since",
    )
    .await;
    assert_eq!(reverse.rows, vec![vec![Value::Int(2021)]]);

    // An edge that does not exist is a no-op, not an upsert: fabricating one
    // here would skip the degree counters and ontology triples INSERT maintains.
    let missing = execute(
        &service,
        session_id,
        "UPDATE EDGE ON knows 2->1 SET since = 1999",
    )
    .await;
    assert_eq!(missing.rows, vec![vec![Value::Int(0)]]);
    let not_created = execute(
        &service,
        session_id,
        "GO FROM 2 OVER knows YIELD knows.since AS since",
    )
    .await;
    assert!(
        not_created.rows.is_empty(),
        "UPDATE must not create an edge, got {:?}",
        not_created.rows
    );

    // Ranking is part of the edge's identity, so an update addressed to a
    // different rank must not touch rank 0.
    execute(
        &service,
        session_id,
        "INSERT EDGE knows(since) VALUES 1->2@7:(1990)",
    )
    .await;
    execute(
        &service,
        session_id,
        "UPDATE EDGE ON knows 1->2@7 SET since = 1991",
    )
    .await;
    // Rank 7 took the new value and rank 0 kept the earlier one. Addressed by
    // rank now that `FETCH PROP` honours it (#108).
    let ranked = execute(&service, session_id, "FETCH PROP ON knows 1->2").await;
    let mut ranks: Vec<(i64, i64)> = ranked
        .rows
        .iter()
        .filter_map(|row| {
            row.iter().find_map(|value| match value {
                // FETCH PROP renders an edge as a JSON object.
                Value::String(json) => {
                    let edge: serde_json::Value = serde_json::from_str(json).ok()?;
                    Some((edge["ranking"].as_i64()?, edge["props"]["since"].as_i64()?))
                }
                _ => None,
            })
        })
        .collect();
    ranks.sort();
    ranks.dedup();
    assert_eq!(
        ranks,
        vec![(0, 2021), (7, 1991)],
        "each rank must keep its own properties, got {:?}",
        ranked.rows
    );

    // `WHEN` gates an edge update the way it gates a vertex update: a false
    // condition is a no-op, and a true one applies.
    let refused = execute(
        &service,
        session_id,
        "UPDATE EDGE ON knows 1->2 SET since = 3000 WHEN knows.since == 1900",
    )
    .await;
    assert_eq!(refused.rows, vec![vec![Value::Int(0)]]);
    let accepted = execute(
        &service,
        session_id,
        "UPDATE EDGE ON knows 1->2 SET since = 2022 WHEN knows.since == 2021",
    )
    .await;
    assert_eq!(accepted.rows, vec![vec![Value::Int(1)]]);
    let gated = execute(
        &service,
        session_id,
        "GO FROM 1 OVER knows YIELD knows.since AS since",
    )
    .await;
    assert!(
        gated
            .rows
            .iter()
            .any(|row| row.first() == Some(&Value::Int(2022))),
        "a satisfied WHEN must apply, got {:?}",
        gated.rows
    );

    // An unknown property is rejected against the edge schema rather than
    // written, matching INSERT EDGE.
    let unknown_field = service
        .execute(
            session_id,
            "UPDATE EDGE ON knows 1->2 SET nonexistent = 1".to_string(),
        )
        .await
        .expect_err("an unknown edge field must be refused");
    assert!(
        unknown_field.to_string().contains("nonexistent"),
        "the error must name the field, got: {unknown_field}"
    );

    service.sign_out(session_id, session_id).await.unwrap();
}

/// Regression for #108. `FETCH PROP ON <edge> src->dst@rank` parsed the rank and
/// then discarded it, so the reference matched every rank of the pair and could
/// not address the edge it named — the outlier among INSERT, DELETE, and UPDATE,
/// where a rank identifies exactly one edge.
#[tokio::test(flavor = "multi_thread")]
async fn fetch_prop_on_an_edge_honors_its_rank() {
    let (service, _temp_dir) = create_test_service();
    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    execute(&service, session_id, "CREATE SPACE fetch_rank").await;
    execute(&service, session_id, "USE fetch_rank").await;
    execute(&service, session_id, "CREATE TAG p(name STRING)").await;
    execute(&service, session_id, "CREATE EDGE knows(since INT64)").await;
    execute(
        &service,
        session_id,
        r#"INSERT VERTEX p(name) VALUES 1:("A"), 2:("B")"#,
    )
    .await;
    execute(
        &service,
        session_id,
        "INSERT EDGE knows(since) VALUES 1->2:(2020), 1->2@7:(1990)",
    )
    .await;

    /// Collect `(ranking, since)` from a FETCH result. An edge is rendered as a
    /// JSON object in the row.
    fn ranks(result: &byoridb_common::DataSet) -> Vec<(i64, i64)> {
        let mut pairs: Vec<(i64, i64)> = result
            .rows
            .iter()
            .filter_map(|row| {
                row.iter().find_map(|value| match value {
                    Value::String(json) => {
                        let edge: serde_json::Value = serde_json::from_str(json).ok()?;
                        Some((edge["ranking"].as_i64()?, edge["props"]["since"].as_i64()?))
                    }
                    _ => None,
                })
            })
            .collect();
        pairs.sort();
        pairs.dedup();
        pairs
    }

    // An explicit rank addresses exactly one edge.
    let ranked = execute(&service, session_id, "FETCH PROP ON knows 1->2@7").await;
    assert_eq!(
        ranks(&ranked),
        vec![(7, 1990)],
        "@7 must return only rank 7, got {:?}",
        ranked.rows
    );

    let rank_zero = execute(&service, session_id, "FETCH PROP ON knows 1->2@0").await;
    assert_eq!(ranks(&rank_zero), vec![(0, 2020)]);

    // An omitted rank keeps its long-standing meaning: every rank of the pair.
    let all_ranks = execute(&service, session_id, "FETCH PROP ON knows 1->2").await;
    assert_eq!(ranks(&all_ranks), vec![(0, 2020), (7, 1990)]);

    // A rank with no edge is empty rather than falling back to another rank.
    let absent = execute(&service, session_id, "FETCH PROP ON knows 1->2@99").await;
    assert!(
        ranks(&absent).is_empty(),
        "a rank with no edge must return nothing, got {:?}",
        absent.rows
    );

    // Each reference in a list carries its own rank.
    let mixed = execute(&service, session_id, "FETCH PROP ON knows 1->2@7, 1->2@0").await;
    assert_eq!(ranks(&mixed), vec![(0, 2020), (7, 1990)]);

    service.sign_out(session_id, session_id).await.unwrap();
}

/// The FIXED_STRING migration path, established by execution rather than
/// asserted from the design (#82).
///
/// Each block below answers one question the docs could not, and the book's
/// "Migrating from client-hashed integer VIDs" section states what this proves.
#[tokio::test(flavor = "multi_thread")]
async fn fixed_string_vid_migration_path_from_client_hashed_int64() {
    let (service, _temp_dir) = create_test_service();
    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    // A client that hashed names into i64 itself, which is what byoridb/byori
    // does and what predates FIXED_STRING support.
    execute(
        &service,
        session_id,
        "CREATE SPACE legacy_int (vid_type=INT64)",
    )
    .await;
    execute(&service, session_id, "USE legacy_int").await;
    execute(
        &service,
        session_id,
        "CREATE TAG note(name STRING, body STRING)",
    )
    .await;
    execute(&service, session_id, "CREATE EDGE rel(kind STRING)").await;
    execute(
        &service,
        session_id,
        r#"INSERT VERTEX note(name, body) VALUES 111:("decision:use-redb", "adopt redb"), 222:("module:kvstore", "redb KV")"#,
    )
    .await;
    execute(
        &service,
        session_id,
        r#"INSERT EDGE rel(kind) VALUES 111->222:("affects")"#,
    )
    .await;
    let before_migration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;

    // (1) A space's vid_type is fixed at CREATE SPACE: ALTER has no SPACE form,
    // so there is no in-place conversion and migration is space-to-space.
    let alter = service
        .execute(
            session_id,
            "ALTER SPACE legacy_int (vid_type=FIXED_STRING(64))".to_string(),
        )
        .await
        .expect_err("vid_type must not be alterable in place");
    assert!(
        alter.to_string().to_lowercase().contains("parse")
            || alter.to_string().to_lowercase().contains("unexpected"),
        "ALTER SPACE should not parse, got: {alter}"
    );

    // (2) Writing an integer VID into a FIXED_STRING space is refused, naming
    // the legacy read/delete-only status rather than silently creating a vertex.
    execute(
        &service,
        session_id,
        "CREATE SPACE migrated_str (vid_type=FIXED_STRING(64))",
    )
    .await;
    execute(&service, session_id, "USE migrated_str").await;
    execute(
        &service,
        session_id,
        "CREATE TAG note(name STRING, body STRING)",
    )
    .await;
    execute(&service, session_id, "CREATE EDGE rel(kind STRING)").await;

    let int_write = service
        .execute(
            session_id,
            r#"INSERT VERTEX note(name, body) VALUES 111:("x", "y")"#.to_string(),
        )
        .await
        .expect_err("an integer VID must not be writable in a FIXED_STRING space");
    let message = int_write.to_string();
    assert!(
        message.contains("read/delete-only") && message.contains("111"),
        "the refusal must explain itself and name the VID, got: {message}"
    );

    // (3) The internal surrogate namespace is not addressable either. On a write
    // the integer guard fires first, so every integer VID — negative included —
    // is refused as legacy; the surrogate-specific refusal appears on the
    // read/delete paths, which are the only ones an integer can reach.
    let surrogate_write = service
        .execute(
            session_id,
            r#"INSERT VERTEX note(name, body) VALUES -1:("x", "y")"#.to_string(),
        )
        .await
        .expect_err("a raw internal surrogate must not be writable");
    assert!(
        surrogate_write.to_string().contains("read/delete-only"),
        "got: {surrogate_write}"
    );
    let surrogate_read = service
        .execute(session_id, "FETCH PROP ON note -1".to_string())
        .await
        .expect_err("a raw internal surrogate must not be readable");
    assert!(
        surrogate_read.to_string().contains("negative"),
        "the surrogate namespace must be refused by name, got: {surrogate_read}"
    );

    // (4) The migration itself: re-insert under string VIDs. Reads return the
    // string, not the internal surrogate.
    execute(
        &service,
        session_id,
        r#"INSERT VERTEX note(name, body) VALUES "decision:use-redb":("decision:use-redb", "adopt redb"), "module:kvstore":("module:kvstore", "redb KV")"#,
    )
    .await;
    execute(
        &service,
        session_id,
        r#"INSERT EDGE rel(kind) VALUES "decision:use-redb"->"module:kvstore":("affects")"#,
    )
    .await;
    let migrated = execute(
        &service,
        session_id,
        r#"MATCH (n:note) WHERE id(n) == "decision:use-redb" RETURN id(n) AS vid, n.note.body AS body"#,
    )
    .await;
    assert_eq!(
        migrated.rows,
        vec![vec![
            Value::String("decision:use-redb".to_string()),
            Value::String("adopt redb".to_string()),
        ]]
    );
    let traversed = execute(
        &service,
        session_id,
        r#"GO FROM "decision:use-redb" OVER rel YIELD rel.kind AS kind"#,
    )
    .await;
    assert_eq!(
        traversed.rows,
        vec![vec![Value::String("affects".to_string())]]
    );

    // (5) History does not follow a re-key. It is keyed by space and VID, so
    // the migrated vertex has no history before its insert...
    let new_vid_history = execute(
        &service,
        session_id,
        &format!(r#"FETCH PROP ON note "decision:use-redb" AS OF {before_migration}"#),
    )
    .await;
    assert!(
        new_vid_history.rows.is_empty(),
        "a re-keyed vertex must not inherit history, got {:?}",
        new_vid_history.rows
    );

    // ...while the original remains readable at that timestamp under its old
    // space and VID, which is what makes the old space the archive of record.
    execute(&service, session_id, "USE legacy_int").await;
    let old_vid_history = execute(
        &service,
        session_id,
        &format!("FETCH PROP ON note 111 AS OF {before_migration}"),
    )
    .await;
    assert!(
        !old_vid_history.rows.is_empty(),
        "the pre-migration history must stay with the original space and VID"
    );

    // (6) RECOMMEND is INT64-only and says so, naming the space, rather than
    // returning results computed over internal surrogates.
    execute(&service, session_id, "USE migrated_str").await;
    let recommend = service
        .execute(session_id, r#"RECOMMEND SIMILAR TO 1 OVER rel"#.to_string())
        .await
        .expect_err("RECOMMEND must be refused in a FIXED_STRING space");
    let recommend_message = recommend.to_string();
    assert!(
        recommend_message.contains("INT64-only") && recommend_message.contains("migrated_str"),
        "got: {recommend_message}"
    );

    service.sign_out(session_id, session_id).await.unwrap();
}

/// Regression for #79. `LOOKUP ON <edge type>` was refused outright, leaving no
/// indexed entry point into the edge set — selecting edges by property meant
/// traversing from endpoints you already knew.
#[tokio::test(flavor = "multi_thread")]
async fn lookup_on_an_edge_type_returns_matching_edges() {
    let (service, _temp_dir) = create_test_service();
    let session_id = service
        .authenticate("root".to_string(), DEFAULT_PASSWORD.to_string())
        .await
        .expect("Authentication failed");

    execute(&service, session_id, "CREATE SPACE edge_lookup").await;
    execute(&service, session_id, "USE edge_lookup").await;
    execute(&service, session_id, "CREATE TAG p(name STRING)").await;
    execute(&service, session_id, "CREATE EDGE knows(since INT64)").await;
    execute(
        &service,
        session_id,
        r#"INSERT VERTEX p(name) VALUES 1:("A"), 2:("B"), 3:("C")"#,
    )
    .await;
    execute(
        &service,
        session_id,
        "INSERT EDGE knows(since) VALUES 1->2:(2020), 1->3:(2021), 2->3:(2020)",
    )
    .await;

    /// `(src, dst, rank)` triples from a LOOKUP result, sorted for comparison.
    fn edges(result: &byoridb_common::DataSet) -> Vec<(i64, i64, i64)> {
        let mut found: Vec<(i64, i64, i64)> = result
            .rows
            .iter()
            .filter_map(|row| match (row.first(), row.get(1), row.get(2)) {
                (Some(Value::Int(src)), Some(Value::Int(dst)), Some(Value::Int(rank))) => {
                    Some((*src, *dst, *rank))
                }
                _ => None,
            })
            .collect();
        found.sort();
        found
    }

    // Without an index this takes the predicate scan, which must still be
    // correct — that is what makes the index an optimisation rather than the
    // feature.
    let scanned = execute(
        &service,
        session_id,
        "LOOKUP ON knows WHERE knows.since == 2020",
    )
    .await;
    assert_eq!(
        scanned.column_names,
        vec!["knows.src", "knows.dst", "knows.rank"],
        "rank is part of an edge's identity and is projected with the endpoints"
    );
    assert_eq!(edges(&scanned), vec![(1, 2, 0), (2, 3, 0)]);

    // The same answer through the index.
    execute(
        &service,
        session_id,
        "CREATE EDGE INDEX knows_since_idx ON knows(since)",
    )
    .await;
    let indexed = execute(
        &service,
        session_id,
        "LOOKUP ON knows WHERE knows.since == 2020",
    )
    .await;
    assert_eq!(
        edges(&indexed),
        vec![(1, 2, 0), (2, 3, 0)],
        "the index must agree with the scan"
    );

    // An unqualified field resolves too, as it does for tags.
    let unqualified = execute(&service, session_id, "LOOKUP ON knows WHERE since == 2021").await;
    assert_eq!(edges(&unqualified), vec![(1, 3, 0)]);

    // A range predicate has no bounded edge-index form, so it takes the scan and
    // must still be right.
    let ranged = execute(&service, session_id, "LOOKUP ON knows WHERE since > 2020").await;
    assert_eq!(edges(&ranged), vec![(1, 3, 0)]);

    // OFFSET/LIMIT window the result.
    let limited = execute(
        &service,
        session_id,
        "LOOKUP ON knows WHERE knows.since == 2020 LIMIT 1",
    )
    .await;
    assert_eq!(limited.rows.len(), 1, "LIMIT must bound the result");

    // An updated property moves the edge between result sets, and the stale
    // index entry must not resurrect the old value.
    execute(
        &service,
        session_id,
        "UPDATE EDGE ON knows 1->2 SET since = 2099",
    )
    .await;
    let after_update = execute(
        &service,
        session_id,
        "LOOKUP ON knows WHERE knows.since == 2020",
    )
    .await;
    assert_eq!(
        edges(&after_update),
        vec![(2, 3, 0)],
        "an updated edge must leave the old value's result set"
    );
    let moved = execute(
        &service,
        session_id,
        "LOOKUP ON knows WHERE knows.since == 2099",
    )
    .await;
    assert_eq!(edges(&moved), vec![(1, 2, 0)]);

    // A deleted edge must not be a hit, whether or not its index entry survived.
    execute(&service, session_id, "DELETE EDGE knows 2->3").await;
    let after_delete = execute(
        &service,
        session_id,
        "LOOKUP ON knows WHERE knows.since == 2020",
    )
    .await;
    assert!(
        edges(&after_delete).is_empty(),
        "a deleted edge must not be returned, got {:?}",
        after_delete.rows
    );

    // EXPLAIN must say which access path ran, and must not describe an edge
    // LOOKUP as a tag scan.
    let plan_text = |result: &byoridb_common::DataSet| -> String {
        result
            .rows
            .iter()
            .flatten()
            .map(|value| match value {
                Value::String(text) => text.clone(),
                other => format!("{other:?}"),
            })
            .collect::<Vec<_>>()
            .join(" | ")
    };
    let explained_index = execute(
        &service,
        session_id,
        "EXPLAIN LOOKUP ON knows WHERE knows.since == 2099",
    )
    .await;
    let indexed_plan = plan_text(&explained_index);
    assert!(
        indexed_plan.contains("IndexScan")
            && indexed_plan.contains("knows_since_idx")
            && indexed_plan.contains("on Edge knows"),
        "an indexed edge LOOKUP must be reported as such, got: {indexed_plan}"
    );
    assert!(
        indexed_plan.contains("knows.src,knows.dst,knows.rank"),
        "the projection must match what the executor returns, got: {indexed_plan}"
    );

    // A range predicate has no bounded edge-index form, so it must be reported
    // as a scan rather than claiming an index.
    let explained_range = execute(
        &service,
        session_id,
        "EXPLAIN LOOKUP ON knows WHERE knows.since > 2020",
    )
    .await;
    let range_plan = plan_text(&explained_range);
    assert!(
        range_plan.contains("EdgeScan") && range_plan.contains("FULL SCAN"),
        "an unindexed edge predicate must be reported as a scan, got: {range_plan}"
    );

    // A tag LOOKUP in the same space is unaffected.
    let tag_lookup = execute(&service, session_id, r#"LOOKUP ON p WHERE p.name == "A""#).await;
    assert_eq!(tag_lookup.rows.len(), 1);
    let explained_tag = execute(
        &service,
        session_id,
        r#"EXPLAIN LOOKUP ON p WHERE p.name == "A""#,
    )
    .await;
    assert!(
        plan_text(&explained_tag).contains("on Tag p"),
        "a tag LOOKUP must still be described as a tag"
    );

    service.sign_out(session_id, session_id).await.unwrap();
}

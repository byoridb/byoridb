// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Basic usage example for ByoriDB

use byoridb_common::datatypes::vertex::Tag;
use byoridb_common::{Edge, Value, Vertex};
use byoridb_graph::service::GraphService;
use byoridb_kvstore::{KVStoreOptions, RocksdbKVStore};
use byoridb_parser::parse;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("ByoriDB - Basic Usage Example");
    println!("======================================\n");

    // Example 1: Creating data structures
    println!("1. Creating graph data structures:");
    create_data_structures();

    // Example 2: Parsing nGQL queries
    println!("\n2. Parsing nGQL queries:");
    parse_queries();

    // Example 3: Using the graph service
    println!("\n3. Using Graph Service:");
    use_graph_service().await?;

    Ok(())
}

fn create_data_structures() {
    // Create a vertex
    let vertex = Vertex::new(Value::Int(1001));
    println!("Created vertex: {}", vertex.to_string());

    // Create a vertex with tags
    let mut player = Vertex::new(Value::Int(1001));
    let mut props = std::collections::HashMap::new();
    props.insert("name".to_string(), Value::String("Alice".to_string()));
    props.insert("age".to_string(), Value::Int(25));
    player.add_tag(Tag::with_props("player".to_string(), props));
    println!("Player vertex: {}", player.to_string());

    // Create an edge
    let edge = Edge::new(Value::Int(1001), Value::Int(1002), 1, "follows", 0);
    println!("Created edge: {}", edge.to_string());
}

fn parse_queries() {
    let queries = vec![
        "SHOW SPACES",
        "CREATE SPACE my_space",
        "USE my_space",
        "CREATE TAG player(name string, age int64)",
        "CREATE EDGE follows(weight double)",
        "INSERT VERTEX player VALUES 1001:(\"Alice\", 25)",
    ];

    for query in queries {
        match parse(query) {
            Ok(_stmt) => println!("Parsed: {}", query),
            Err(e) => println!("Parse error: {} - {}", query, e),
        }
    }
}

async fn use_graph_service() -> Result<(), Box<dyn std::error::Error>> {
    // Create a temporary KVStore
    let temp_dir = std::env::temp_dir().join("byoridb_example_kvstore");
    let _ = std::fs::remove_dir_all(&temp_dir); // Clean up if exists
    let kvstore = Arc::new(RocksdbKVStore::open(&temp_dir, KVStoreOptions::default())?);

    // Create a graph service
    let graph_service = GraphService::new(kvstore);

    // Authenticate and create a session
    let session_id = graph_service
        .authenticate("user".to_string(), "pass".to_string())
        .await?;
    println!("Created session: {}", session_id);

    // Execute a query
    let result = graph_service
        .execute(session_id, "SHOW SPACES".to_string())
        .await?;
    println!("Query result: {} rows", result.row_count());

    // Sign out
    graph_service.sign_out(session_id, session_id).await;
    println!("Session closed");

    // Cleanup
    let _ = std::fs::remove_dir_all(temp_dir);

    Ok(())
}

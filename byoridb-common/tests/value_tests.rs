// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use byoridb_common::{datatypes::vertex::Tag, Edge, Value, Vertex};
use std::collections::HashMap;

#[test]
fn test_value_creation() {
    let int_val = Value::Int(42);
    assert!(int_val.is_int());
    assert!(int_val.is_numeric());
    assert_eq!(int_val.to_string(), "42");

    let string_val = Value::String("hello".to_string());
    assert!(string_val.is_str());
    assert_eq!(string_val.to_string(), "hello");

    let bool_val = Value::Bool(true);
    assert!(bool_val.is_bool());
    assert_eq!(bool_val.to_string(), "true");

    let null_val = Value::Null(byoridb_common::types::NullType::Null);
    assert!(null_val.is_null());
}

#[test]
fn test_value_conversions() {
    let int_from_i32: Value = 42.into();
    assert!(int_from_i32.is_int());

    let string_from_str: Value = "test".into();
    assert!(string_from_str.is_str());

    let bool_from_bool: Value = true.into();
    assert!(bool_from_bool.is_bool());
}

#[test]
fn test_vertex_creation() {
    let vertex = Vertex::new(Value::Int(1001));
    assert_eq!(vertex.to_string(), "1001[]");

    let mut player = Vertex::new(Value::Int(1001));
    let mut props = HashMap::new();
    props.insert("name".to_string(), Value::String("Alice".to_string()));
    props.insert("age".to_string(), Value::Int(25));
    player.add_tag(Tag::with_props("player".to_string(), props));

    assert!(player.contains("name"));
    assert!(player.contains("age"));
}

#[test]
fn test_edge_creation() {
    let edge = Edge::new(Value::Int(1001), Value::Int(1002), 1, "follows", 0);
    assert_eq!(edge.name, "follows");
    assert_eq!(edge.edge_type, 1);

    let mut props = HashMap::new();
    props.insert("weight".to_string(), Value::Float(0.8));
    let weighted_edge =
        Edge::with_props(Value::Int(1001), Value::Int(1002), 2, "knows", 12345, props);
    assert!(weighted_edge.contains("weight"));
    assert_eq!(weighted_edge.value("weight").unwrap(), &Value::Float(0.8));
}

#[test]
fn test_value_equality() {
    let v1 = Value::Int(42);
    let v2 = Value::Int(42);
    assert_eq!(v1, v2);

    let v3 = Value::Int(43);
    assert_ne!(v1, v3);

    let vertex1 = Vertex::new(Value::Int(1));
    let vertex2 = Vertex::new(Value::Int(1));
    assert_eq!(vertex1, vertex2);
}

#[test]
fn test_value_hash() {
    use std::collections::HashSet;
    let mut set = HashSet::new();

    set.insert(Value::Int(1));
    set.insert(Value::Int(2));
    set.insert(Value::String("test".to_string()));

    assert_eq!(set.len(), 3);
    assert!(set.contains(&Value::Int(1)));
    assert!(set.contains(&Value::String("test".to_string())));
}

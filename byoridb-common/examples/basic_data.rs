// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Basic usage example for ByoriDB common data types

use byoridb_common::datatypes::vertex::Tag;
use byoridb_common::{Edge, Value, Vertex};
use std::collections::HashMap;

fn main() {
    println!("ByoriDB - Basic Data Types Example");
    println!("===========================================\n");

    // Example 1: Creating values
    println!("1. Creating Values:");
    let int_val = Value::Int(42);
    let string_val = Value::String("Hello".to_string());
    let bool_val = Value::Bool(true);
    println!("Integer: {}", int_val.to_string());
    println!("String: {}", string_val.to_string());
    println!("Boolean: {}", bool_val.to_string());

    // Example 2: Creating vertices
    println!("\n2. Creating Vertices:");
    let vertex = Vertex::new(Value::Int(1001));
    println!("Simple vertex: {}", vertex.to_string());

    let mut player = Vertex::new(Value::Int(1001));
    let mut props = HashMap::new();
    props.insert("name".to_string(), Value::String("Alice".to_string()));
    props.insert("age".to_string(), Value::Int(25));
    props.insert("score".to_string(), Value::Float(95.5));
    player.add_tag(Tag::with_props("player".to_string(), props));
    println!("Player vertex: {}", player.to_string());

    // Example 3: Creating edges
    println!("\n3. Creating Edges:");
    let edge = Edge::new(Value::Int(1001), Value::Int(1002), 1, "follows", 0);
    println!("Follows edge: {}", edge.to_string());

    let mut edge_props = HashMap::new();
    edge_props.insert("weight".to_string(), Value::Float(0.8));
    let weighted_edge = Edge::with_props(
        Value::Int(1001),
        Value::Int(1002),
        2,
        "knows",
        12345,
        edge_props,
    );
    println!("Weighted edge: {}", weighted_edge.to_string());

    // Example 4: Type conversions
    println!("\n4. Type Conversions:");
    let val: Value = 42.into();
    println!("Integer from i32: {}", val.to_string());

    let val2: Value = "Hello Rust".into();
    println!("String from &str: {}", val2.to_string());

    let val3: Value = true.into();
    println!("Bool from bool: {}", val3.to_string());

    // Example 5: Value operations
    println!("\n5. Value Type Checking:");
    println!("Is int a number? {}", val.is_numeric());
    println!("Is int an int? {}", val.is_int());
    println!("Is str a string? {}", val2.is_str());
    println!("Is bool a bool? {}", val3.is_bool());

    println!("\n✅ All examples completed successfully!");
}

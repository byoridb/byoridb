// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Performance benchmarks for ByoriDB

#![allow(clippy::approx_constant)]

use byoridb_common::filter::{CompareOp, FilterExpr};
use byoridb_common::{datatypes::vertex::Tag, Edge, Value, Vertex};
use byoridb_executor::{ArenaPool, ExecutionPlanBuilder, QueryArena};
use byoridb_parser::parse;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use std::collections::HashMap;

fn benchmark_vertex_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("vertex_creation");

    for size in [10, 100, 1000].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            b.iter(|| {
                let mut vertex = Vertex::new(Value::Int(size));
                let mut props = HashMap::new();
                for i in 0..size {
                    props.insert(format!("prop{}", i), Value::Int(i));
                }
                vertex.add_tag(Tag::with_props("test".to_string(), props));
                black_box(vertex);
            });
        });
    }

    group.finish();
}

fn benchmark_edge_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("edge_creation");

    for size in [10, 100, 1000].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            b.iter(|| {
                let mut props = HashMap::new();
                for i in 0..size {
                    props.insert(format!("prop{}", i), Value::Int(i));
                }
                let edge = Edge::with_props(Value::Int(1), Value::Int(2), 1, "test", 0, props);
                black_box(edge);
            });
        });
    }

    group.finish();
}

fn benchmark_query_parsing(c: &mut Criterion) {
    let queries = vec![
        "SHOW SPACES",
        "USE my_space",
        "CREATE TAG person(name string, age int64)",
        "INSERT VERTEX person VALUES 1:(\"Alice\", 25)",
        "GO FROM 1 OVER follows YIELD follows._dst AS dst",
        "MATCH (v:person) RETURN v",
        "LOOKUP ON person WHERE person.name == \"Alice\"",
    ];

    let mut group = c.benchmark_group("query_parsing");

    for query in queries {
        group.bench_with_input(query, &query, |b, q| {
            b.iter(|| {
                let result = parse(black_box(q));
                let _ = black_box(result);
            });
        });
    }

    group.finish();
}

fn benchmark_execution_plan_building(c: &mut Criterion) {
    let queries = vec![
        "SHOW SPACES",
        "USE my_space",
        "CREATE TAG person(name string)",
    ];

    let mut group = c.benchmark_group("execution_plan_building");

    for query in queries {
        if let Ok(stmt) = parse(query) {
            group.bench_with_input(query, &stmt, |b, s| {
                b.iter(|| {
                    let result = ExecutionPlanBuilder::build(black_box(s.clone()));
                    let _ = black_box(result);
                });
            });
        }
    }

    group.finish();
}

fn benchmark_value_serialization(c: &mut Criterion) {
    let values = vec![
        Value::String("test string".to_string()),
        Value::Int(42),
        Value::Float(3.14159),
        Value::Bool(true),
    ];

    let mut group = c.benchmark_group("value_serialization");

    for value in values {
        let name = format!("{:?}", std::mem::discriminant(&value));
        group.bench_with_input(name, &value, |b, v| {
            b.iter(|| {
                let result = serde_json::to_string(black_box(v));
                let _ = black_box(result);
            });
        });
    }

    group.finish();
}

fn benchmark_value_deserialization(c: &mut Criterion) {
    let json_strings = vec![
        r#"{"String":"test"}"#,
        r#"{"Int":42}"#,
        r#"{"Float":3.14}"#,
        r#"{"Bool":true}"#,
    ];

    let mut group = c.benchmark_group("value_deserialization");

    for json in json_strings {
        group.bench_with_input(json, &json, |b, j| {
            b.iter(|| {
                let result: Result<Value, _> = serde_json::from_str(black_box(j));
                let _ = black_box(result);
            });
        });
    }

    group.finish();
}

fn benchmark_complex_graph_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("graph_operations");

    // Create vertices with properties
    group.bench_function("create_vertex_with_10_props", |b| {
        b.iter(|| {
            let mut vertex = Vertex::new(Value::Int(1));
            let mut props = HashMap::new();
            for i in 0..10 {
                props.insert(format!("prop{}", i), Value::Int(i));
            }
            vertex.add_tag(Tag::with_props("test".to_string(), props));
            black_box(vertex);
        });
    });

    // Create multiple vertices
    group.bench_function("create_100_vertices", |b| {
        b.iter(|| {
            let vertices: Vec<Vertex> = (0..100)
                .map(|i| {
                    let mut v = Vertex::new(Value::Int(i));
                    let mut props = HashMap::new();
                    props.insert("id".to_string(), Value::Int(i));
                    v.add_tag(Tag::with_props("test".to_string(), props));
                    v
                })
                .collect();
            black_box(vertices);
        });
    });

    group.finish();
}

/// Benchmark arena allocation vs standard allocation
fn benchmark_arena_allocation(c: &mut Criterion) {
    let mut group = c.benchmark_group("arena_allocation");

    // Standard allocation: create many small objects
    group.bench_function("std_alloc_1000_i64", |b| {
        b.iter(|| {
            let values: Vec<Box<i64>> = (0..1000).map(|i| Box::new(black_box(i))).collect();
            black_box(values);
        });
    });

    // Arena allocation: create many small objects in arena
    group.bench_function("arena_alloc_1000_i64", |b| {
        b.iter(|| {
            let arena = QueryArena::new();
            let values: Vec<&i64> = (0..1000).map(|i| arena.alloc(black_box(i))).collect();
            black_box(values);
            // Arena dropped here, all memory freed at once
        });
    });

    // Standard allocation: create string slices
    group.bench_function("std_alloc_100_strings", |b| {
        b.iter(|| {
            let strings: Vec<String> = (0..100).map(|i| format!("test_string_{}", i)).collect();
            black_box(strings);
        });
    });

    // Arena allocation: create string slices in arena
    group.bench_function("arena_alloc_100_strings", |b| {
        b.iter(|| {
            let arena = QueryArena::new();
            let strings: Vec<&str> = (0..100)
                .map(|i| {
                    let s = format!("test_string_{}", i);
                    arena.alloc_str(&s)
                })
                .collect();
            black_box(strings);
        });
    });

    // Test arena pool reuse
    group.bench_function("arena_pool_get_put_100", |b| {
        let pool = ArenaPool::new(16);
        b.iter(|| {
            for _ in 0..100 {
                let arena = pool.get();
                arena.alloc(42i64);
                arena.alloc_str("test");
                pool.put(arena);
            }
        });
    });

    group.finish();
}

/// Benchmark filter expression evaluation
fn benchmark_filter_evaluation(c: &mut Criterion) {
    let mut group = c.benchmark_group("filter_evaluation");

    // Create test data
    let test_values: Vec<Value> = vec![
        Value::Int(42),
        Value::String("test".to_string()),
        Value::Float(3.14),
        Value::Bool(true),
    ];

    // Simple equality filter
    group.bench_function("filter_eq_int", |b| {
        let filter = FilterExpr::Compare {
            field: "age".to_string(),
            op: CompareOp::Eq,
            value: Value::Int(42),
        };
        b.iter(|| {
            let _ = black_box(&filter);
            black_box(&test_values[0]);
        });
    });

    // Complex AND filter
    group.bench_function("filter_and_two_conditions", |b| {
        let filter = FilterExpr::And(
            Box::new(FilterExpr::Compare {
                field: "age".to_string(),
                op: CompareOp::Gt,
                value: Value::Int(18),
            }),
            Box::new(FilterExpr::Compare {
                field: "age".to_string(),
                op: CompareOp::Lt,
                value: Value::Int(65),
            }),
        );
        b.iter(|| {
            black_box(&filter);
        });
    });

    // Filter creation benchmark
    group.bench_function("filter_create_complex", |b| {
        b.iter(|| {
            let filter = FilterExpr::And(
                Box::new(FilterExpr::Compare {
                    field: "name".to_string(),
                    op: CompareOp::Eq,
                    value: Value::String("Alice".to_string()),
                }),
                Box::new(FilterExpr::Or(
                    Box::new(FilterExpr::Compare {
                        field: "age".to_string(),
                        op: CompareOp::Gt,
                        value: Value::Int(18),
                    }),
                    Box::new(FilterExpr::Compare {
                        field: "active".to_string(),
                        op: CompareOp::Eq,
                        value: Value::Bool(true),
                    }),
                )),
            );
            black_box(filter);
        });
    });

    // InList filter
    group.bench_function("filter_in_list_10_items", |b| {
        let values: Vec<Value> = (0..10).map(Value::Int).collect();
        b.iter(|| {
            let filter = FilterExpr::InList {
                field: "id".to_string(),
                values: values.clone(),
            };
            black_box(filter);
        });
    });

    group.finish();
}

/// Benchmark slice operations with arena vs standard Vec
fn benchmark_slice_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("slice_operations");

    let source_data: Vec<i64> = (0..10000).collect();

    // Standard Vec copy
    group.bench_function("vec_copy_10000", |b| {
        b.iter(|| {
            let copy: Vec<i64> = source_data.to_vec();
            black_box(copy);
        });
    });

    // Arena slice copy
    group.bench_function("arena_slice_copy_10000", |b| {
        b.iter(|| {
            let arena = QueryArena::new();
            let slice = arena.alloc_slice_copy(&source_data);
            black_box(slice);
        });
    });

    // Multiple small allocations
    group.bench_function("vec_100_small_allocs", |b| {
        b.iter(|| {
            let vecs: Vec<Vec<i64>> = (0..100).map(|i| vec![i; 10]).collect();
            black_box(vecs);
        });
    });

    group.bench_function("arena_100_small_allocs", |b| {
        b.iter(|| {
            let arena = QueryArena::new();
            let slices: Vec<&[i64]> = (0..100)
                .map(|i| {
                    let data: Vec<i64> = vec![i; 10];
                    arena.alloc_slice_copy(&data)
                })
                .collect();
            black_box(slices);
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    benchmark_vertex_creation,
    benchmark_edge_creation,
    benchmark_query_parsing,
    benchmark_execution_plan_building,
    benchmark_value_serialization,
    benchmark_value_deserialization,
    benchmark_complex_graph_operations,
    benchmark_arena_allocation,
    benchmark_filter_evaluation,
    benchmark_slice_operations
);

criterion_main!(benches);

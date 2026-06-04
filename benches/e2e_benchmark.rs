// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! E2E Performance benchmarks for ByoriDB
//!
//! This benchmark tests the full query execution pipeline including:
//! - Data import (INSERT VERTEX/EDGE)
//! - Point queries (FETCH)
//! - Graph traversal (GO)
//! - Index lookups (LOOKUP)

use byoridb_graph::service::GraphService;
use byoridb_kvstore::{KVStoreOptions, RocksdbKVStore};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::sync::Arc;
use tokio::runtime::Runtime;

const BENCH_ROOT_PASSWORD: &str = "byoridb-bench-root-password";

/// Helper to create a test environment
struct TestEnv {
    graph_service: GraphService,
    session_id: i64,
    _temp_dir: tempfile::TempDir,
}

impl TestEnv {
    fn new() -> Self {
        std::env::set_var("BYORIDB_ROOT_PASSWORD", BENCH_ROOT_PASSWORD);

        let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
        let kvstore = Arc::new(
            RocksdbKVStore::open(temp_dir.path(), KVStoreOptions::default())
                .expect("Failed to create KVStore"),
        );
        let graph_service = GraphService::new(kvstore);

        let rt = Runtime::new().unwrap();
        let session_id = rt.block_on(async {
            graph_service
                .authenticate("root".to_string(), BENCH_ROOT_PASSWORD.to_string())
                .await
                .expect("Failed to authenticate")
        });

        TestEnv {
            graph_service,
            session_id,
            _temp_dir: temp_dir,
        }
    }

    fn execute(
        &self,
        query: &str,
    ) -> Result<byoridb_common::DataSet, Box<dyn std::error::Error + Send + Sync>> {
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            self.graph_service
                .execute(self.session_id, query.to_string())
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        })
    }

    fn setup_space(&self, space_name: &str) {
        let _ = self.execute(&format!("CREATE SPACE {}", space_name));
        let _ = self.execute(&format!("USE {}", space_name));
        let _ = self.execute("CREATE TAG person(name STRING, age INT64, score FLOAT)");
        let _ = self.execute("CREATE EDGE follows(weight FLOAT, since INT64)");
    }
}

/// Benchmark data import performance
fn benchmark_data_import(c: &mut Criterion) {
    let mut group = c.benchmark_group("e2e_data_import");
    group.sample_size(10); // Reduce sample size for slower benchmarks

    for vertex_count in [100, 1000, 10000].iter() {
        group.throughput(Throughput::Elements(*vertex_count as u64));
        group.bench_with_input(
            BenchmarkId::new("insert_vertices", vertex_count),
            vertex_count,
            |b, &count| {
                b.iter_with_setup(
                    || {
                        let env = TestEnv::new();
                        env.setup_space("bench_import");
                        env
                    },
                    |env| {
                        // Batch insert vertices
                        let batch_size = 100.min(count);
                        for batch_start in (1..=count).step_by(batch_size) {
                            let batch_end = (batch_start + batch_size - 1).min(count);
                            let values: Vec<String> = (batch_start..=batch_end)
                                .map(|i| {
                                    format!(
                                        "{}:(\"user{}\", {}, {})",
                                        i,
                                        i,
                                        i % 100,
                                        i as f64 * 0.1
                                    )
                                })
                                .collect();
                            let query = format!(
                                "INSERT VERTEX person(name, age, score) VALUES {}",
                                values.join(", ")
                            );
                            let _ = env.execute(&query);
                        }
                        black_box(count)
                    },
                );
            },
        );
    }

    group.finish();
}

/// Benchmark point query (FETCH) performance
fn benchmark_fetch_queries(c: &mut Criterion) {
    let mut group = c.benchmark_group("e2e_fetch");
    group.sample_size(20);

    // Setup: Create environment with pre-loaded data
    let env = TestEnv::new();
    env.setup_space("bench_fetch");

    // Pre-load 1000 vertices
    for batch_start in (1..=1000).step_by(100) {
        let batch_end = batch_start + 99;
        let values: Vec<String> = (batch_start..=batch_end)
            .map(|i| format!("{}:(\"user{}\", {}, {})", i, i, i % 100, i as f64 * 0.1))
            .collect();
        let query = format!(
            "INSERT VERTEX person(name, age, score) VALUES {}",
            values.join(", ")
        );
        let _ = env.execute(&query);
    }

    // Benchmark single vertex fetch
    group.bench_function("fetch_single_vertex", |b| {
        b.iter(|| {
            let result = env.execute("FETCH PROP ON person 500");
            black_box(result)
        });
    });

    // Benchmark batch vertex fetch
    for batch_size in [10, 50, 100].iter() {
        group.bench_with_input(
            BenchmarkId::new("fetch_batch", batch_size),
            batch_size,
            |b, &size| {
                let vids: Vec<String> = (1..=size).map(|i| i.to_string()).collect();
                let query = format!("FETCH PROP ON person {}", vids.join(", "));
                b.iter(|| {
                    let result = env.execute(&query);
                    black_box(result)
                });
            },
        );
    }

    group.finish();
}

/// Benchmark graph traversal (GO) performance
fn benchmark_go_queries(c: &mut Criterion) {
    let mut group = c.benchmark_group("e2e_go_traversal");
    group.sample_size(20);

    // Setup: Create environment with graph data
    let env = TestEnv::new();
    env.setup_space("bench_go");

    // Pre-load vertices
    for batch_start in (1..=500).step_by(100) {
        let batch_end = (batch_start + 99).min(500);
        let values: Vec<String> = (batch_start..=batch_end)
            .map(|i| format!("{}:(\"user{}\", {}, {})", i, i, i % 100, i as f64 * 0.1))
            .collect();
        let query = format!(
            "INSERT VERTEX person(name, age, score) VALUES {}",
            values.join(", ")
        );
        let _ = env.execute(&query);
    }

    // Pre-load edges (create a graph where each vertex follows next 5 vertices)
    for batch_start in (1..=495).step_by(50) {
        let batch_end = (batch_start + 49).min(495);
        let edges: Vec<String> = (batch_start..=batch_end)
            .flat_map(|i| {
                (1..=5).map(move |j| {
                    let dst = ((i + j - 1) % 500) + 1;
                    format!("{}->{}:(0.{}, 2020)", i, dst, j)
                })
            })
            .collect();
        let query = format!(
            "INSERT EDGE follows(weight, since) VALUES {}",
            edges.join(", ")
        );
        let _ = env.execute(&query);
    }

    // Benchmark 1-hop traversal
    group.bench_function("go_1_hop", |b| {
        b.iter(|| {
            let result = env.execute("GO FROM 1 OVER follows YIELD follows._dst AS dst");
            black_box(result)
        });
    });

    // Benchmark 2-hop traversal
    group.bench_function("go_2_hops", |b| {
        b.iter(|| {
            let result = env.execute("GO 2 STEPS FROM 1 OVER follows YIELD follows._dst AS dst");
            black_box(result)
        });
    });

    // Benchmark 3-hop traversal
    group.bench_function("go_3_hops", |b| {
        b.iter(|| {
            let result = env.execute("GO 3 STEPS FROM 1 OVER follows YIELD follows._dst AS dst");
            black_box(result)
        });
    });

    // Benchmark with WHERE filter
    group.bench_function("go_1_hop_filtered", |b| {
        b.iter(|| {
            let result = env.execute(
                "GO FROM 1 OVER follows WHERE follows.weight > 0.3 YIELD follows._dst AS dst",
            );
            black_box(result)
        });
    });

    group.finish();
}

/// Benchmark LOOKUP queries
fn benchmark_lookup_queries(c: &mut Criterion) {
    let mut group = c.benchmark_group("e2e_lookup");
    group.sample_size(20);

    // Setup: Create environment with data
    let env = TestEnv::new();
    env.setup_space("bench_lookup");

    // Pre-load vertices
    for batch_start in (1..=1000).step_by(100) {
        let batch_end = batch_start + 99;
        let values: Vec<String> = (batch_start..=batch_end)
            .map(|i| format!("{}:(\"user{}\", {}, {})", i, i, i % 100, i as f64 * 0.1))
            .collect();
        let query = format!(
            "INSERT VERTEX person(name, age, score) VALUES {}",
            values.join(", ")
        );
        let _ = env.execute(&query);
    }

    // Benchmark equality lookup
    group.bench_function("lookup_eq", |b| {
        b.iter(|| {
            let result = env.execute("LOOKUP ON person WHERE person.age == 25");
            black_box(result)
        });
    });

    // Benchmark range lookup
    group.bench_function("lookup_range", |b| {
        b.iter(|| {
            let result = env.execute("LOOKUP ON person WHERE person.age > 20 AND person.age < 30");
            black_box(result)
        });
    });

    // Benchmark string equality lookup
    group.bench_function("lookup_string_eq", |b| {
        b.iter(|| {
            let result = env.execute("LOOKUP ON person WHERE person.name == \"user500\"");
            black_box(result)
        });
    });

    group.finish();
}

/// Benchmark query parsing + planning + execution (full pipeline)
fn benchmark_full_pipeline(c: &mut Criterion) {
    let mut group = c.benchmark_group("e2e_full_pipeline");
    group.sample_size(50);

    let env = TestEnv::new();
    env.setup_space("bench_pipeline");

    // Pre-load some data
    for batch_start in (1..=100).step_by(100) {
        let batch_end = batch_start + 99;
        let values: Vec<String> = (batch_start..=batch_end)
            .map(|i| format!("{}:(\"user{}\", {}, {})", i, i, i % 100, i as f64 * 0.1))
            .collect();
        let query = format!(
            "INSERT VERTEX person(name, age, score) VALUES {}",
            values.join(", ")
        );
        let _ = env.execute(&query);
    }

    // Benchmark different query types
    let queries = vec![
        ("show_spaces", "SHOW SPACES"),
        ("show_tags", "SHOW TAGS"),
        ("fetch_single", "FETCH PROP ON person 50"),
        ("lookup_simple", "LOOKUP ON person WHERE person.age == 50"),
    ];

    for (name, query) in queries {
        group.bench_function(name, |b| {
            b.iter(|| {
                let result = env.execute(query);
                black_box(result)
            });
        });
    }

    group.finish();
}

/// Benchmark concurrent query execution
fn benchmark_concurrent_queries(c: &mut Criterion) {
    let mut group = c.benchmark_group("e2e_concurrent");
    group.sample_size(10);

    let env = TestEnv::new();
    env.setup_space("bench_concurrent");

    // Pre-load data
    for batch_start in (1..=500).step_by(100) {
        let batch_end = (batch_start + 99).min(500);
        let values: Vec<String> = (batch_start..=batch_end)
            .map(|i| format!("{}:(\"user{}\", {}, {})", i, i, i % 100, i as f64 * 0.1))
            .collect();
        let query = format!(
            "INSERT VERTEX person(name, age, score) VALUES {}",
            values.join(", ")
        );
        let _ = env.execute(&query);
    }

    // Benchmark sequential vs concurrent execution
    group.bench_function("sequential_10_queries", |b| {
        b.iter(|| {
            for i in 1..=10 {
                let result = env.execute(&format!("FETCH PROP ON person {}", i * 10));
                let _ = black_box(result);
            }
        });
    });

    group.finish();
}

criterion_group!(
    e2e_benches,
    benchmark_data_import,
    benchmark_fetch_queries,
    benchmark_go_queries,
    benchmark_lookup_queries,
    benchmark_full_pipeline,
    benchmark_concurrent_queries,
);

criterion_main!(e2e_benches);

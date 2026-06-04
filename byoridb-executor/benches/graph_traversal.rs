// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Graph traversal benchmarks.
//!
//! Measures `GO`, BFS shortest path, weighted (Dijkstra) shortest path, and
//! MATCH execution time across synthetic graph topologies. Each scenario
//! runs against [`byoridb_kvstore::store::MemoryKVStore`] so the numbers reflect
//! executor/algorithm cost, not disk I/O — this matches the Phase 7 goal of
//! tracking optimization deltas in a deterministic environment.
//!
//! Topologies:
//!   - **chain**: linear path `0 -> 1 -> ... -> n-1`. Worst case for BFS
//!     depth and best case for low-fanout traversal.
//!   - **star**: 1 hub with `n-1` spokes. The hub is the high-degree vertex
//!     that exposes `Vec` materialization cost on the neighbor scan path.
//!   - **scale_free**: preferential-attachment construction with a fixed
//!     seed. Approximates a real graph with a few high-degree hubs.

use byoridb_codec::{EdgeData, VertexCodec, VertexData};
use byoridb_common::Value;
use byoridb_executor::algo::{bfs_shortest_path, dijkstra_shortest_path, get_neighbors};
use byoridb_executor::context::ExecutionContext;
use byoridb_kvstore::{store::MemoryKVStore, KVStore};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::runtime::Runtime;

const SPACE: &str = "bench";

fn make_runtime() -> Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn make_ctx(kvstore: Arc<MemoryKVStore>) -> ExecutionContext {
    ExecutionContext::new(kvstore).with_space(SPACE.to_string())
}

async fn put_vertex(kvstore: &Arc<MemoryKVStore>, vid: i64) {
    let vertex = VertexData { vid, tags: vec![] };
    let key = format!("{}:vertex:{}", SPACE, vid);
    let bytes = VertexCodec::encode_vertex(&vertex).unwrap();
    kvstore.put(key.as_bytes(), &bytes).await.unwrap();
}

async fn put_edge(
    kvstore: &Arc<MemoryKVStore>,
    src: i64,
    dst: i64,
    edge_type: &str,
    ranking: i64,
    weight: Option<f64>,
) {
    let mut properties = HashMap::new();
    if let Some(w) = weight {
        properties.insert("cost".to_string(), Value::Float(w));
    }
    let edge = EdgeData {
        src_vid: src,
        dst_vid: dst,
        edge_type: edge_type.to_string(),
        ranking,
        properties,
    };
    let key = format!("{}:edge:{}:{}:{}:{}", SPACE, src, edge_type, dst, ranking);
    let bytes = VertexCodec::encode_edge(&edge).unwrap();
    kvstore.put(key.as_bytes(), &bytes).await.unwrap();
}

/// Build a chain graph `0 -> 1 -> 2 -> ... -> n-1`.
async fn build_chain(n: usize, weighted: bool) -> Arc<MemoryKVStore> {
    let kvstore = Arc::new(MemoryKVStore::new());
    for vid in 0..n as i64 {
        put_vertex(&kvstore, vid).await;
    }
    for src in 0..(n - 1) as i64 {
        let weight = if weighted {
            Some(1.0 + (src as f64) * 0.001)
        } else {
            None
        };
        put_edge(&kvstore, src, src + 1, "follow", 0, weight).await;
    }
    kvstore
}

/// Build a star graph with `n-1` spokes radiating out of vertex `0`.
async fn build_star(n: usize) -> Arc<MemoryKVStore> {
    let kvstore = Arc::new(MemoryKVStore::new());
    put_vertex(&kvstore, 0).await;
    for dst in 1..n as i64 {
        put_vertex(&kvstore, dst).await;
        put_edge(&kvstore, 0, dst, "follow", dst, None).await;
    }
    kvstore
}

/// Build a scale-free-ish graph via preferential attachment. Deterministic:
/// each new vertex attaches to `m` existing vertices, choosing each with
/// probability proportional to its current degree. RNG seed is fixed.
async fn build_scale_free(n: usize, m: usize) -> Arc<MemoryKVStore> {
    let kvstore = Arc::new(MemoryKVStore::new());
    let mut degree = vec![0u64; n];
    // Seed: a tiny clique among the first `m+1` vertices.
    for vid in 0..=m as i64 {
        put_vertex(&kvstore, vid).await;
    }
    for src in 0..=m as i64 {
        for dst in 0..=m as i64 {
            if src != dst {
                put_edge(&kvstore, src, dst, "follow", dst, None).await;
                degree[src as usize] += 1;
            }
        }
    }

    // Simple LCG so the bench is reproducible without an rng crate.
    let mut state: u64 = 0xdead_beef_cafe_babe;
    let mut next_u64 = || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        state
    };

    for new_vid in (m + 1) as i64..n as i64 {
        put_vertex(&kvstore, new_vid).await;
        let total: u64 = degree[..new_vid as usize].iter().sum::<u64>().max(1);
        let mut attached = std::collections::HashSet::new();
        while attached.len() < m {
            let mut pick = next_u64() % total;
            let mut chosen = 0i64;
            for (i, d) in degree[..new_vid as usize].iter().enumerate() {
                if pick < *d {
                    chosen = i as i64;
                    break;
                }
                pick -= *d;
            }
            if attached.insert(chosen) {
                put_edge(&kvstore, new_vid, chosen, "follow", chosen, None).await;
                degree[new_vid as usize] += 1;
                degree[chosen as usize] += 1;
            }
        }
    }

    kvstore
}

fn bench_get_neighbors(c: &mut Criterion) {
    let rt = make_runtime();
    let mut group = c.benchmark_group("get_neighbors");

    for &n in &[64usize, 1024, 16384] {
        let kvstore = rt.block_on(build_star(n));
        let ctx = make_ctx(kvstore);
        group.throughput(Throughput::Elements((n - 1) as u64));
        group.bench_with_input(BenchmarkId::new("star_hub", n), &n, |b, _| {
            b.to_async(&rt).iter(|| async {
                let neighbors = get_neighbors(&ctx, SPACE, 0, &["follow".to_string()])
                    .await
                    .unwrap();
                criterion::black_box(neighbors.len())
            });
        });
    }

    group.finish();
}

fn bench_bfs(c: &mut Criterion) {
    let rt = make_runtime();
    let mut group = c.benchmark_group("bfs_shortest_path");

    for &n in &[16usize, 256, 4096] {
        let kvstore = rt.block_on(build_chain(n, false));
        let ctx = make_ctx(kvstore);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::new("chain_far", n), &n, |b, _| {
            b.to_async(&rt).iter(|| async {
                let (path, metrics) =
                    bfs_shortest_path(&ctx, 0, (n - 1) as i64, &["follow".to_string()], n)
                        .await
                        .unwrap();
                criterion::black_box((path, metrics.scanned_edges))
            });
        });
    }

    // Unreachable: chain of length n, target outside the chain (separate VID).
    let n = 4096usize;
    let kvstore = rt.block_on(build_chain(n, false));
    let ctx = make_ctx(kvstore);
    group.bench_function(BenchmarkId::new("chain_unreachable", n), |b| {
        b.to_async(&rt).iter(|| async {
            let (path, metrics) =
                bfs_shortest_path(&ctx, 0, (n + 100) as i64, &["follow".to_string()], n)
                    .await
                    .unwrap();
            criterion::black_box((path, metrics.visited_vertices))
        });
    });

    group.finish();
}

fn bench_dijkstra(c: &mut Criterion) {
    let rt = make_runtime();
    let mut group = c.benchmark_group("dijkstra_shortest_path");

    for &n in &[16usize, 256, 4096] {
        let kvstore = rt.block_on(build_chain(n, true));
        let ctx = make_ctx(kvstore);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::new("chain_weighted", n), &n, |b, _| {
            b.to_async(&rt).iter(|| async {
                let (path, metrics) = dijkstra_shortest_path(
                    &ctx,
                    0,
                    (n - 1) as i64,
                    &["follow".to_string()],
                    "cost",
                )
                .await
                .unwrap();
                criterion::black_box((path, metrics.decoded_edges))
            });
        });
    }

    group.finish();
}

fn bench_scale_free(c: &mut Criterion) {
    let rt = make_runtime();
    let mut group = c.benchmark_group("scale_free");

    let n = 1024usize;
    let m = 4usize;
    let kvstore = rt.block_on(build_scale_free(n, m));
    let ctx = make_ctx(kvstore);

    group.bench_function(BenchmarkId::new("bfs_random_target", n), |b| {
        b.to_async(&rt).iter(|| async {
            // Pick target deterministically — hub-ish vertex 0 is reachable.
            let (path, metrics) =
                bfs_shortest_path(&ctx, (n - 1) as i64, 0, &["follow".to_string()], n)
                    .await
                    .unwrap();
            criterion::black_box((path, metrics.scanned_edges))
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_get_neighbors,
    bench_bfs,
    bench_dijkstra,
    bench_scale_free
);
criterion_main!(benches);

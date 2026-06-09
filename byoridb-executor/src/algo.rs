// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Graph algorithms (BFS, Dijkstra)

use crate::context::ExecutionContext;
use crate::error::{ExecutionError, Result};
use byoridb_codec::{EdgeData, VertexCodec};
use futures::StreamExt;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};

#[derive(Debug, Clone)]
pub struct GraphNeighbor {
    pub dst: i64,
    pub edge: EdgeData,
}

pub type NeighborVisitor = Box<dyn FnMut(GraphNeighbor) -> bool + Send>;

/// Per-query traversal counters. Populated by BFS/Dijkstra and returned to
/// the caller alongside the path so executor instrumentation can log
/// scanned/decoded volume without re-running the traversal.
#[derive(Debug, Default, Clone, Copy)]
pub struct TraversalMetrics {
    /// Vertices pulled from the queue (BFS) or heap (Dijkstra).
    pub visited_vertices: u64,
    /// Edge entries returned by `scan_with_filter` (post edge-type filter,
    /// pre decode). Counted via `visit_neighbors_limited`.
    pub scanned_edges: u64,
    /// Edges that successfully decoded into `EdgeData` and were considered
    /// for relaxation/expansion.
    pub decoded_edges: u64,
    /// Maximum size the BFS queue / Dijkstra heap reached during the run.
    pub max_frontier_size: u64,
    /// True iff the traversal stopped because it hit `max_traversal_nodes`.
    pub cap_reached: bool,
}

impl TraversalMetrics {
    fn record_frontier(&mut self, size: usize) {
        let size = size as u64;
        if size > self.max_frontier_size {
            self.max_frontier_size = size;
        }
    }
}

pub async fn get_neighbors(
    ctx: &ExecutionContext,
    space: &str,
    src: i64,
    edge_types: &[String],
) -> Result<Vec<GraphNeighbor>> {
    get_neighbors_limited(ctx, space, src, edge_types, None).await
}

/// Find vertices that have an edge pointing **into** `dst` (reverse traversal).
///
/// Reads the reverse-edge index `{space}:in-edge:{dst}:{edge_type}:{src}:{ranking}`,
/// whose value is the denormalized edge payload. This is a prefix scan over a
/// single vertex's in-edges — **O(in-degree)**, symmetric to the outgoing
/// `get_neighbors_counted` scan. INSERT/DELETE EDGE maintain the index
/// (see `executor::dml`).
///
/// Note: edges inserted before the reverse index existed have no in-edge
/// entries, so a space must be (re)loaded after this index was introduced for
/// reverse traversal to return results (see PLAN.md O-1).
pub async fn get_incoming_neighbors(
    ctx: &ExecutionContext,
    space: &str,
    dst: i64,
    edge_types: &[String],
) -> Result<Vec<GraphNeighbor>> {
    let in_edge_prefix = crate::key::SchemaKey::in_edge_data_dst_prefix(space, dst);
    let edge_type_set: HashSet<String> = edge_types.iter().cloned().collect();

    // Reverse key places edge_type on segment 3, same as the forward key, so
    // edge_type_from_key filters both directions identically.
    let results = ctx
        .kvstore
        .scan_with_filter(
            &in_edge_prefix,
            Box::new(move |key, _| {
                edge_type_set.is_empty()
                    || edge_type_from_key(key)
                        .map(|edge_type| edge_type_set.contains(edge_type))
                        .unwrap_or(false)
            }),
            None,
        )
        .await?;

    let mut neighbors = Vec::with_capacity(results.len());
    for (_key, value) in results {
        if let Ok(edge_data) = VertexCodec::decode_edge(&value) {
            // Reverse traversal yields the *source* vertex as the neighbor.
            neighbors.push(GraphNeighbor {
                dst: edge_data.src_vid,
                edge: edge_data,
            });
        }
    }

    Ok(neighbors)
}

pub async fn get_neighbors_limited(
    ctx: &ExecutionContext,
    space: &str,
    src: i64,
    edge_types: &[String],
    limit: Option<usize>,
) -> Result<Vec<GraphNeighbor>> {
    let mut scanned = 0;
    let mut decoded = 0;
    get_neighbors_counted(
        ctx,
        space,
        src,
        edge_types,
        limit,
        &mut scanned,
        &mut decoded,
    )
    .await
}

/// Like [`get_neighbors_limited`] but records the number of edge rows the
/// scan returned (`scanned`) and the number that decoded into `EdgeData`
/// (`decoded`). Used by BFS/Dijkstra to populate [`TraversalMetrics`].
pub(crate) async fn get_neighbors_counted(
    ctx: &ExecutionContext,
    space: &str,
    src: i64,
    edge_types: &[String],
    limit: Option<usize>,
    scanned: &mut u64,
    decoded: &mut u64,
) -> Result<Vec<GraphNeighbor>> {
    let edge_prefix = format!("{}:edge:{}:", space, src);
    let edge_type_set: HashSet<String> = edge_types.iter().cloned().collect();

    let results = ctx
        .kvstore
        .scan_with_filter(
            edge_prefix.as_bytes(),
            Box::new(move |key, _| {
                edge_type_set.is_empty()
                    || edge_type_from_key(key)
                        .map(|edge_type| edge_type_set.contains(edge_type))
                        .unwrap_or(false)
            }),
            limit,
        )
        .await?;

    *scanned += results.len() as u64;
    let mut neighbors = Vec::with_capacity(results.len());
    for (_key, value) in results {
        if let Ok(edge) = byoridb_codec::VertexCodec::decode_edge(&value) {
            *decoded += 1;
            neighbors.push(GraphNeighbor {
                dst: edge.dst_vid,
                edge,
            });
        }
    }

    Ok(neighbors)
}

pub async fn visit_neighbors_limited(
    ctx: &ExecutionContext,
    space: &str,
    src: i64,
    edge_types: &[String],
    limit: Option<usize>,
    mut visitor: NeighborVisitor,
) -> Result<usize> {
    let edge_prefix = format!("{}:edge:{}:", space, src);
    let edge_type_set: HashSet<String> = edge_types.iter().cloned().collect();

    let visited = ctx
        .kvstore
        .scan_with_filter_visit(
            edge_prefix.as_bytes(),
            Box::new(move |key, _| {
                edge_type_set.is_empty()
                    || edge_type_from_key(key)
                        .map(|edge_type| edge_type_set.contains(edge_type))
                        .unwrap_or(false)
            }),
            Box::new(move |_, value| {
                if let Ok(edge) = byoridb_codec::VertexCodec::decode_edge(value) {
                    visitor(GraphNeighbor {
                        dst: edge.dst_vid,
                        edge,
                    })
                } else {
                    true
                }
            }),
            limit,
        )
        .await?;

    Ok(visited)
}

fn edge_type_from_key(key: &[u8]) -> Option<&str> {
    let key_str = std::str::from_utf8(key).ok()?;
    key_str.split(':').nth(3)
}

/// Breadth-First Search for unweighted shortest path.
///
/// Uses the streaming `KVStore::scan_stream` API plus
/// [`VertexCodec::decode_edge_dst`] so each neighbor is decoded in-place
/// from the stream without materializing a `Vec<GraphNeighbor>`. This is
/// the hot path for high-degree (celebrity-style) vertices — the previous
/// implementation allocated one full `EdgeData` per spoke even though BFS
/// only needs `dst_vid`.
///
/// Returns the shortest path along with [`TraversalMetrics`].
/// `metrics.cap_reached` is set when the traversal stopped because of
/// `ctx.config.max_traversal_nodes`.
pub async fn bfs_shortest_path(
    ctx: &ExecutionContext,
    start: i64,
    target: i64,
    edge_types: &[String],
    max_steps: usize,
) -> Result<(Option<Vec<i64>>, TraversalMetrics)> {
    let mut metrics = TraversalMetrics::default();

    if start == target {
        metrics.visited_vertices = 1;
        return Ok((Some(vec![start]), metrics));
    }

    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    let mut parent = HashMap::new();

    queue.push_back((start, 0));
    visited.insert(start);
    metrics.record_frontier(queue.len());

    let max_nodes = ctx.config.max_traversal_nodes;
    let space = ctx.space.as_ref().unwrap_or(&"default".to_string()).clone();
    let edge_type_set: HashSet<&str> = edge_types.iter().map(|s| s.as_str()).collect();

    while let Some((current, step)) = queue.pop_front() {
        metrics.visited_vertices += 1;
        if max_nodes > 0 && metrics.visited_vertices > max_nodes as u64 {
            metrics.cap_reached = true;
            break;
        }

        if step >= max_steps {
            continue;
        }

        let edge_prefix = format!("{}:edge:{}:", space, current);
        let mut stream = ctx.kvstore.scan_stream(edge_prefix.as_bytes()).await?;
        while let Some(item) = stream.next().await {
            let (key, value) = item?;
            if !edge_type_set.is_empty() {
                let edge_type = match edge_type_from_key(&key) {
                    Some(t) => t,
                    None => continue,
                };
                if !edge_type_set.contains(edge_type) {
                    continue;
                }
            }
            metrics.scanned_edges += 1;
            let dst = match VertexCodec::decode_edge_dst(&value) {
                Ok(d) => d,
                Err(_) => continue,
            };
            metrics.decoded_edges += 1;
            if !visited.contains(&dst) {
                visited.insert(dst);
                parent.insert(dst, current);
                if dst == target {
                    // Drop the stream early — the backing iterator task
                    // sees the channel close and exits without scanning
                    // the rest of this vertex's neighbors.
                    drop(stream);
                    return Ok((Some(reconstruct_path(parent, start, target)), metrics));
                }
                queue.push_back((dst, step + 1));
                metrics.record_frontier(queue.len());
            }
        }
    }

    Ok((None, metrics))
}

/// Dijkstra's Algorithm for weighted shortest path.
///
/// Streams edges per visited vertex via `KVStore::scan_stream` and decodes
/// each as a full [`EdgeData`] (Dijkstra needs the weight property, so the
/// dst-only fast path isn't applicable). Returns the weighted path along
/// with [`TraversalMetrics`]. Negative edge weights are rejected with
/// `InvalidOperation` rather than silently skipped.
pub async fn dijkstra_shortest_path(
    ctx: &ExecutionContext,
    start: i64,
    target: i64,
    edge_types: &[String],
    weight_prop: &str,
) -> Result<(Option<(Vec<i64>, f64)>, TraversalMetrics)> {
    let mut metrics = TraversalMetrics::default();
    let mut dist = HashMap::new();
    let mut parent = HashMap::new();
    let mut heap = BinaryHeap::new();

    dist.insert(start, 0.0_f64);
    heap.push(Reverse(DisplayableOrderedFloat(0.0, start)));
    metrics.record_frontier(heap.len());

    let space = ctx.space.as_ref().unwrap_or(&"default".to_string()).clone();
    let max_nodes = ctx.config.max_traversal_nodes;
    let edge_type_set: HashSet<&str> = edge_types.iter().map(|s| s.as_str()).collect();

    while let Some(Reverse(DisplayableOrderedFloat(d, u))) = heap.pop() {
        metrics.visited_vertices += 1;
        if max_nodes > 0 && metrics.visited_vertices > max_nodes as u64 {
            metrics.cap_reached = true;
            break;
        }

        if u == target {
            return Ok((Some((reconstruct_path(parent, start, target), d)), metrics));
        }

        if d > *dist.get(&u).unwrap_or(&f64::INFINITY) {
            continue;
        }

        let edge_prefix = format!("{}:edge:{}:", space, u);
        let mut stream = ctx.kvstore.scan_stream(edge_prefix.as_bytes()).await?;
        while let Some(item) = stream.next().await {
            let (key, value) = item?;
            if !edge_type_set.is_empty() {
                let edge_type = match edge_type_from_key(&key) {
                    Some(t) => t,
                    None => continue,
                };
                if !edge_type_set.contains(edge_type) {
                    continue;
                }
            }
            metrics.scanned_edges += 1;
            let edge = match VertexCodec::decode_edge(&value) {
                Ok(e) => e,
                Err(_) => continue,
            };
            metrics.decoded_edges += 1;

            let weight = edge
                .properties
                .get(weight_prop)
                .and_then(|v| match v {
                    byoridb_common::Value::Float(f) => Some(*f),
                    byoridb_common::Value::Int(i) => Some(*i as f64),
                    _ => None,
                })
                .unwrap_or(1.0);

            if weight < 0.0 {
                return Err(ExecutionError::InvalidOperation(
                    "Dijkstra shortest path does not support negative edge weights".to_string(),
                ));
            }

            let dst = edge.dst_vid;
            let next_dist = d + weight;
            if next_dist < *dist.get(&dst).unwrap_or(&f64::INFINITY) {
                dist.insert(dst, next_dist);
                parent.insert(dst, u);
                heap.push(Reverse(DisplayableOrderedFloat(next_dist, dst)));
                metrics.record_frontier(heap.len());
            }
        }
    }

    Ok((None, metrics))
}

fn reconstruct_path(parent: HashMap<i64, i64>, start: i64, target: i64) -> Vec<i64> {
    let mut path = vec![target];
    let mut curr = target;
    while curr != start {
        if let Some(&p) = parent.get(&curr) {
            curr = p;
            path.push(curr);
        } else {
            break; // Should not happen if path exists
        }
    }
    path.reverse();
    path
}

// Wrapper for f64 to implement Ord
#[derive(PartialEq, Debug)]
struct DisplayableOrderedFloat(f64, i64);

impl Eq for DisplayableOrderedFloat {}

impl Ord for DisplayableOrderedFloat {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0
            .partial_cmp(&other.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    }
}

impl PartialOrd for DisplayableOrderedFloat {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use byoridb_codec::VertexCodec;
    use byoridb_kvstore::store::MemoryKVStore;
    use std::sync::Arc;

    fn test_context() -> ExecutionContext {
        ExecutionContext::new(Arc::new(MemoryKVStore::new())).with_space("default".to_string())
    }

    async fn insert_edge(
        ctx: &ExecutionContext,
        src: i64,
        dst: i64,
        edge_type: &str,
        ranking: i64,
    ) {
        let key = format!("default:edge:{}:{}:{}:{}", src, edge_type, dst, ranking);
        let edge = EdgeData {
            src_vid: src,
            dst_vid: dst,
            edge_type: edge_type.to_string(),
            ranking,
            properties: HashMap::new(),
        };
        let data = VertexCodec::encode_edge(&edge).unwrap();
        ctx.kvstore.put(key.as_bytes(), &data).await.unwrap();
    }

    async fn insert_weighted_edge(
        ctx: &ExecutionContext,
        src: i64,
        dst: i64,
        edge_type: &str,
        ranking: i64,
        weight: f64,
    ) {
        let key = format!("default:edge:{}:{}:{}:{}", src, edge_type, dst, ranking);
        let mut properties = HashMap::new();
        properties.insert("cost".to_string(), byoridb_common::Value::Float(weight));
        let edge = EdgeData {
            src_vid: src,
            dst_vid: dst,
            edge_type: edge_type.to_string(),
            ranking,
            properties,
        };
        let data = VertexCodec::encode_edge(&edge).unwrap();
        ctx.kvstore.put(key.as_bytes(), &data).await.unwrap();
    }

    #[tokio::test]
    async fn bfs_shortest_path_handles_cycles() {
        let ctx = test_context();
        insert_edge(&ctx, 1, 2, "follow", 0).await;
        insert_edge(&ctx, 2, 1, "follow", 0).await;
        insert_edge(&ctx, 2, 3, "follow", 1).await;

        let (path, metrics) = bfs_shortest_path(&ctx, 1, 3, &["follow".to_string()], 10)
            .await
            .unwrap();

        assert_eq!(path, Some(vec![1, 2, 3]));
        assert!(metrics.visited_vertices >= 2);
        assert!(metrics.decoded_edges >= 2);
        assert!(!metrics.cap_reached);
    }

    #[tokio::test]
    async fn bfs_shortest_path_respects_edge_type_filter() {
        let ctx = test_context();
        insert_edge(&ctx, 1, 2, "blocked", 0).await;
        insert_edge(&ctx, 1, 3, "follow", 0).await;

        let (path, _) = bfs_shortest_path(&ctx, 1, 2, &["follow".to_string()], 10)
            .await
            .unwrap();

        assert_eq!(path, None);
    }

    #[tokio::test]
    async fn bfs_shortest_path_respects_configurable_cap() {
        use crate::context::ExecutionConfig;

        let mut ctx = test_context();
        ctx.config = ExecutionConfig {
            max_traversal_nodes: 1,
            ..ExecutionConfig::default()
        };
        insert_edge(&ctx, 1, 2, "follow", 0).await;
        insert_edge(&ctx, 2, 3, "follow", 0).await;

        let (path, metrics) = bfs_shortest_path(&ctx, 1, 3, &["follow".to_string()], 10)
            .await
            .unwrap();

        assert_eq!(path, None);
        assert!(metrics.cap_reached);
    }

    #[tokio::test]
    async fn get_neighbors_limited_caps_decoded_neighbors() {
        let ctx = test_context();
        for dst in 2..=6 {
            insert_edge(&ctx, 1, dst, "follow", dst).await;
        }

        let neighbors = get_neighbors_limited(&ctx, "default", 1, &["follow".to_string()], Some(2))
            .await
            .unwrap();

        assert_eq!(neighbors.len(), 2);
    }

    #[tokio::test]
    async fn visit_neighbors_limited_stops_when_visitor_returns_false() {
        let ctx = test_context();
        for dst in 2..=6 {
            insert_edge(&ctx, 1, dst, "follow", dst).await;
        }

        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let visitor_seen = seen.clone();
        let visited = visit_neighbors_limited(
            &ctx,
            "default",
            1,
            &["follow".to_string()],
            None,
            Box::new(move |neighbor| {
                let mut seen = visitor_seen.lock().unwrap();
                seen.push(neighbor.dst);
                seen.len() < 2
            }),
        )
        .await
        .unwrap();

        assert_eq!(visited, 2);
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn dijkstra_shortest_path_uses_weights() {
        let ctx = test_context();
        insert_weighted_edge(&ctx, 1, 2, "follow", 0, 10.0).await;
        insert_weighted_edge(&ctx, 1, 3, "follow", 1, 1.0).await;
        insert_weighted_edge(&ctx, 3, 2, "follow", 0, 1.0).await;

        let (result, metrics) = dijkstra_shortest_path(&ctx, 1, 2, &["follow".to_string()], "cost")
            .await
            .unwrap();

        assert_eq!(result, Some((vec![1, 3, 2], 2.0)));
        assert!(metrics.visited_vertices >= 2);
        assert!(metrics.decoded_edges >= 2);
    }

    #[tokio::test]
    async fn dijkstra_shortest_path_rejects_negative_weights() {
        let ctx = test_context();
        insert_weighted_edge(&ctx, 1, 2, "follow", 0, -1.0).await;

        let result = dijkstra_shortest_path(&ctx, 1, 2, &["follow".to_string()], "cost").await;

        assert!(matches!(
            result,
            Err(ExecutionError::InvalidOperation(message))
                if message.contains("negative edge weights")
        ));
    }
}

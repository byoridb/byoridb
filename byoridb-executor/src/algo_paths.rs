// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! FIND PATH algorithms: single and all-shortest-path BFS with optional
//! bidirectional expansion (PLAN.md O-2 track 1).
//!
//! Bidirectional (`BIDIRECT`) expansion treats every edge as undirected by
//! scanning both the forward `{space}:edge:{vid}:` prefix and the O-1
//! reverse-edge index `{space}:in-edge:{vid}:`. This is required for graphs
//! whose undirected relations are stored single-direction (e.g. LDBC
//! `knows`). Edges inserted before the in-edge index existed are invisible
//! to reverse expansion — the same space-reload caveat as O-1.

use crate::algo::{edge_type_from_key, TraversalMetrics};
use crate::context::ExecutionContext;
use crate::error::Result;
use byoridb_codec::VertexCodec;
use futures::StreamExt;
use std::collections::{HashMap, HashSet, VecDeque};

/// Collect the distinct neighbor vids of `vid`: destinations of outgoing
/// edges, plus sources of incoming edges when `bidirect`. Only the vid is
/// decoded (fast-path field extraction), never the full `EdgeData`.
async fn collect_neighbor_vids(
    ctx: &ExecutionContext,
    space: &str,
    vid: i64,
    edge_types: &HashSet<&str>,
    bidirect: bool,
    metrics: &mut TraversalMetrics,
) -> Result<Vec<i64>> {
    let mut neighbors = Vec::new();
    let mut seen = HashSet::new();

    let forward_prefix = format!("{}:edge:{}:", space, vid);
    let mut stream = ctx.kvstore.scan_stream(forward_prefix.as_bytes()).await?;
    while let Some(item) = stream.next().await {
        let (key, value) = item?;
        if !edge_types.is_empty() {
            match edge_type_from_key(&key) {
                Some(t) if edge_types.contains(t) => {}
                _ => continue,
            }
        }
        metrics.scanned_edges += 1;
        let dst = match VertexCodec::decode_edge_dst(&value) {
            Ok(d) => d,
            Err(_) => continue,
        };
        metrics.decoded_edges += 1;
        if seen.insert(dst) {
            neighbors.push(dst);
        }
    }

    if bidirect {
        let reverse_prefix = crate::key::SchemaKey::in_edge_data_dst_prefix(space, vid);
        let mut stream = ctx.kvstore.scan_stream(&reverse_prefix).await?;
        while let Some(item) = stream.next().await {
            let (key, value) = item?;
            // The in-edge key places edge_type on segment 3, same as the
            // forward key, so the shared filter applies to both directions.
            if !edge_types.is_empty() {
                match edge_type_from_key(&key) {
                    Some(t) if edge_types.contains(t) => {}
                    _ => continue,
                }
            }
            metrics.scanned_edges += 1;
            // In-edge values are denormalized edge payloads; the neighbor we
            // expand to is the edge's *source*.
            let src = match VertexCodec::decode_edge_src(&value) {
                Ok(s) => s,
                Err(_) => continue,
            };
            metrics.decoded_edges += 1;
            if seen.insert(src) {
                neighbors.push(src);
            }
        }
    }

    Ok(neighbors)
}

/// Single shortest path by hop count, with optional bidirectional expansion.
///
/// Semantics match [`crate::algo::bfs_shortest_path`]: paths are at most
/// `max_steps` hops, `ctx.config.max_traversal_nodes` caps the visited set
/// (`metrics.cap_reached` when hit), and the first path found is returned.
pub async fn shortest_path(
    ctx: &ExecutionContext,
    start: i64,
    target: i64,
    edge_types: &[String],
    max_steps: usize,
    bidirect: bool,
) -> Result<(Option<Vec<i64>>, TraversalMetrics)> {
    let mut metrics = TraversalMetrics::default();

    if start == target {
        metrics.visited_vertices = 1;
        return Ok((Some(vec![start]), metrics));
    }

    let space = ctx.space.as_ref().unwrap_or(&"default".to_string()).clone();
    let edge_type_set: HashSet<&str> = edge_types.iter().map(|s| s.as_str()).collect();
    let max_nodes = ctx.config.max_traversal_nodes;

    let mut visited = HashSet::new();
    let mut parent = HashMap::new();
    let mut queue = VecDeque::new();
    visited.insert(start);
    queue.push_back((start, 0usize));
    metrics.record_frontier(queue.len());

    while let Some((current, step)) = queue.pop_front() {
        metrics.visited_vertices += 1;
        if max_nodes > 0 && metrics.visited_vertices > max_nodes as u64 {
            metrics.cap_reached = true;
            break;
        }
        if step >= max_steps {
            continue;
        }

        let neighbors =
            collect_neighbor_vids(ctx, &space, current, &edge_type_set, bidirect, &mut metrics)
                .await?;
        for dst in neighbors {
            if visited.insert(dst) {
                parent.insert(dst, current);
                if dst == target {
                    return Ok((Some(reconstruct_path(&parent, start, target)), metrics));
                }
                queue.push_back((dst, step + 1));
                metrics.record_frontier(queue.len());
            }
        }
    }

    Ok((None, metrics))
}

/// All minimum-hop paths from `start` to `target` (LDBC IC14-style).
///
/// Level-synchronous BFS builds a multi-parent shortest-path DAG: a vertex
/// first reached at depth `d+1` records *every* depth-`d` predecessor, and
/// the level where `target` appears is fully expanded before stopping so no
/// shortest parent is missed. Paths are then enumerated by walking the DAG
/// backwards from `target`.
///
/// `max_paths` bounds enumeration (`0` = unlimited); the executor compares
/// the result length against the cap to warn about truncation. If
/// `max_traversal_nodes` aborts the search mid-level the parent DAG is
/// incomplete, so no paths are returned and `metrics.cap_reached` is set.
pub async fn all_shortest_paths(
    ctx: &ExecutionContext,
    start: i64,
    target: i64,
    edge_types: &[String],
    max_steps: usize,
    bidirect: bool,
    max_paths: usize,
) -> Result<(Vec<Vec<i64>>, TraversalMetrics)> {
    let mut metrics = TraversalMetrics::default();

    if start == target {
        metrics.visited_vertices = 1;
        return Ok((vec![vec![start]], metrics));
    }

    let space = ctx.space.as_ref().unwrap_or(&"default".to_string()).clone();
    let edge_type_set: HashSet<&str> = edge_types.iter().map(|s| s.as_str()).collect();
    let max_nodes = ctx.config.max_traversal_nodes;

    // depth: first-seen BFS depth per vertex.
    // parents: all depth-(d-1) predecessors of a depth-d vertex.
    let mut depth: HashMap<i64, usize> = HashMap::from([(start, 0)]);
    let mut parents: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut frontier = vec![start];
    let mut current_depth = 0usize;
    let mut found = false;

    'levels: while !frontier.is_empty() && current_depth < max_steps && !found {
        metrics.record_frontier(frontier.len());
        let mut next_frontier = Vec::new();

        for &u in &frontier {
            metrics.visited_vertices += 1;
            if max_nodes > 0 && metrics.visited_vertices > max_nodes as u64 {
                // Aborting mid-level leaves the parent DAG incomplete; a
                // partial all-paths answer is wrong, so return none.
                metrics.cap_reached = true;
                break 'levels;
            }

            let neighbors =
                collect_neighbor_vids(ctx, &space, u, &edge_type_set, bidirect, &mut metrics)
                    .await?;
            for v in neighbors {
                match depth.get(&v) {
                    None => {
                        depth.insert(v, current_depth + 1);
                        parents.insert(v, vec![u]);
                        next_frontier.push(v);
                        if v == target {
                            found = true;
                        }
                    }
                    Some(&dv) if dv == current_depth + 1 => {
                        // Another shortest predecessor discovered within the
                        // same level. `collect_neighbor_vids` dedups per-u,
                        // so no duplicate (u, v) pair can land here.
                        parents.get_mut(&v).expect("depth implies parents").push(u);
                    }
                    _ => {}
                }
            }
        }

        frontier = next_frontier;
        current_depth += 1;
    }

    if !found || metrics.cap_reached {
        return Ok((Vec::new(), metrics));
    }

    // Enumerate paths by DFS over the parent DAG, building backwards from
    // target. Every DAG edge lies on at least one shortest path, so there is
    // no dead-end pruning to do.
    let mut paths = Vec::new();
    let mut stack = vec![vec![target]];
    while let Some(partial) = stack.pop() {
        if max_paths > 0 && paths.len() >= max_paths {
            break;
        }
        let head = *partial.last().expect("partial path is never empty");
        if head == start {
            let mut path = partial;
            path.reverse();
            paths.push(path);
            continue;
        }
        for &p in &parents[&head] {
            let mut extended = partial.clone();
            extended.push(p);
            stack.push(extended);
        }
    }

    Ok((paths, metrics))
}

fn reconstruct_path(parent: &HashMap<i64, i64>, start: i64, target: i64) -> Vec<i64> {
    let mut path = vec![target];
    let mut curr = target;
    while curr != start {
        match parent.get(&curr) {
            Some(&p) => {
                curr = p;
                path.push(curr);
            }
            None => break,
        }
    }
    path.reverse();
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ExecutionConfig;
    use byoridb_codec::EdgeData;
    use byoridb_kvstore::store::MemoryKVStore;
    use std::sync::Arc;

    fn test_context() -> ExecutionContext {
        ExecutionContext::new(Arc::new(MemoryKVStore::new())).with_space("default".to_string())
    }

    /// Insert a forward edge plus its in-edge index entry, mirroring what
    /// INSERT EDGE writes since O-1.
    async fn insert_edge(ctx: &ExecutionContext, src: i64, dst: i64, edge_type: &str) {
        let edge = EdgeData {
            src_vid: src,
            dst_vid: dst,
            edge_type: edge_type.to_string(),
            ranking: 0,
            properties: HashMap::new(),
        };
        let data = VertexCodec::encode_edge(&edge).unwrap();
        let fwd = format!("default:edge:{}:{}:{}:0", src, edge_type, dst);
        ctx.kvstore.put(fwd.as_bytes(), &data).await.unwrap();
        let rev = crate::key::SchemaKey::in_edge_data("default", dst, edge_type, src, 0);
        ctx.kvstore.put(&rev, &data).await.unwrap();
    }

    fn knows() -> Vec<String> {
        vec!["knows".to_string()]
    }

    #[tokio::test]
    async fn shortest_path_directed_matches_bfs() {
        let ctx = test_context();
        insert_edge(&ctx, 1, 2, "knows").await;
        insert_edge(&ctx, 2, 3, "knows").await;

        let (path, _) = shortest_path(&ctx, 1, 3, &knows(), 10, false)
            .await
            .unwrap();
        assert_eq!(path, Some(vec![1, 2, 3]));
    }

    #[tokio::test]
    async fn shortest_path_bidirect_traverses_reverse_edges() {
        let ctx = test_context();
        // Stored as 2->1 and 2->3: from 1, vertex 3 is only reachable by
        // walking 2->1 backwards then 2->3 forwards.
        insert_edge(&ctx, 2, 1, "knows").await;
        insert_edge(&ctx, 2, 3, "knows").await;

        let (directed, _) = shortest_path(&ctx, 1, 3, &knows(), 10, false)
            .await
            .unwrap();
        assert_eq!(directed, None);

        let (bidirect, _) = shortest_path(&ctx, 1, 3, &knows(), 10, true).await.unwrap();
        assert_eq!(bidirect, Some(vec![1, 2, 3]));
    }

    #[tokio::test]
    async fn shortest_path_respects_edge_type_filter() {
        let ctx = test_context();
        insert_edge(&ctx, 1, 2, "blocked").await;

        let (path, _) = shortest_path(&ctx, 1, 2, &knows(), 10, true).await.unwrap();
        assert_eq!(path, None);
    }

    #[tokio::test]
    async fn all_shortest_paths_enumerates_diamond() {
        let ctx = test_context();
        // Diamond: 1 -> {2, 3} -> 4, plus a longer detour 1->5->6->4 that
        // must NOT appear (not minimum-hop).
        insert_edge(&ctx, 1, 2, "knows").await;
        insert_edge(&ctx, 1, 3, "knows").await;
        insert_edge(&ctx, 2, 4, "knows").await;
        insert_edge(&ctx, 3, 4, "knows").await;
        insert_edge(&ctx, 1, 5, "knows").await;
        insert_edge(&ctx, 5, 6, "knows").await;
        insert_edge(&ctx, 6, 4, "knows").await;

        let (mut paths, _) = all_shortest_paths(&ctx, 1, 4, &knows(), 10, false, 0)
            .await
            .unwrap();
        paths.sort();
        assert_eq!(paths, vec![vec![1, 2, 4], vec![1, 3, 4]]);
    }

    #[tokio::test]
    async fn all_shortest_paths_bidirect_single_stored_direction() {
        let ctx = test_context();
        // LDBC-knows-style: undirected semantics, stored one way only.
        // 1-2-4 and 1-3-4 with mixed storage directions.
        insert_edge(&ctx, 2, 1, "knows").await;
        insert_edge(&ctx, 2, 4, "knows").await;
        insert_edge(&ctx, 1, 3, "knows").await;
        insert_edge(&ctx, 4, 3, "knows").await;

        let (mut paths, _) = all_shortest_paths(&ctx, 1, 4, &knows(), 10, true, 0)
            .await
            .unwrap();
        paths.sort();
        assert_eq!(paths, vec![vec![1, 2, 4], vec![1, 3, 4]]);
    }

    #[tokio::test]
    async fn all_shortest_paths_unreachable_returns_empty() {
        let ctx = test_context();
        insert_edge(&ctx, 1, 2, "knows").await;

        let (paths, _) = all_shortest_paths(&ctx, 1, 99, &knows(), 10, true, 0)
            .await
            .unwrap();
        assert!(paths.is_empty());
    }

    #[tokio::test]
    async fn all_shortest_paths_respects_max_steps() {
        let ctx = test_context();
        insert_edge(&ctx, 1, 2, "knows").await;
        insert_edge(&ctx, 2, 3, "knows").await;
        insert_edge(&ctx, 3, 4, "knows").await;

        let (paths, _) = all_shortest_paths(&ctx, 1, 4, &knows(), 2, false, 0)
            .await
            .unwrap();
        assert!(paths.is_empty());

        let (paths, _) = all_shortest_paths(&ctx, 1, 4, &knows(), 3, false, 0)
            .await
            .unwrap();
        assert_eq!(paths, vec![vec![1, 2, 3, 4]]);
    }

    #[tokio::test]
    async fn all_shortest_paths_caps_path_count() {
        let ctx = test_context();
        // 3 parallel middles -> 3 shortest paths; cap at 2.
        for mid in [2, 3, 5] {
            insert_edge(&ctx, 1, mid, "knows").await;
            insert_edge(&ctx, mid, 4, "knows").await;
        }

        let (paths, _) = all_shortest_paths(&ctx, 1, 4, &knows(), 10, false, 2)
            .await
            .unwrap();
        assert_eq!(paths.len(), 2);
    }

    #[tokio::test]
    async fn all_shortest_paths_traversal_cap_returns_no_partial_answer() {
        let mut ctx = test_context();
        ctx.config = ExecutionConfig {
            max_traversal_nodes: 1,
            ..ExecutionConfig::default()
        };
        insert_edge(&ctx, 1, 2, "knows").await;
        insert_edge(&ctx, 2, 3, "knows").await;

        let (paths, metrics) = all_shortest_paths(&ctx, 1, 3, &knows(), 10, false, 0)
            .await
            .unwrap();
        assert!(paths.is_empty());
        assert!(metrics.cap_reached);
    }

    #[tokio::test]
    async fn all_shortest_paths_handles_cycles() {
        let ctx = test_context();
        insert_edge(&ctx, 1, 2, "knows").await;
        insert_edge(&ctx, 2, 1, "knows").await;
        insert_edge(&ctx, 2, 3, "knows").await;

        let (paths, _) = all_shortest_paths(&ctx, 1, 3, &knows(), 10, true, 0)
            .await
            .unwrap();
        assert_eq!(paths, vec![vec![1, 2, 3]]);
    }

    #[tokio::test]
    async fn shortest_path_start_equals_target() {
        let ctx = test_context();
        let (path, _) = shortest_path(&ctx, 7, 7, &knows(), 10, true).await.unwrap();
        assert_eq!(path, Some(vec![7]));

        let (paths, _) = all_shortest_paths(&ctx, 7, 7, &knows(), 10, true, 0)
            .await
            .unwrap();
        assert_eq!(paths, vec![vec![7]]);
    }
}

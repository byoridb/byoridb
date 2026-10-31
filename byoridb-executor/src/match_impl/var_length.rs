// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Variable-length edge traversal for MATCH `*min..max` (PLAN.md O-2 track 2).
//!
//! Semantics (stage 1):
//! - A ranged edge `-[:t*min..max]->` matches every vertex reachable from the
//!   current vertex via `min..=max` hops over matching edges. The result is
//!   the set of **distinct terminal vertices** — one binding per terminal,
//!   not one per path (transitive-closure semantics; Cypher would emit one
//!   row per path).
//! - Cycle policy: a path never revisits a vertex (visited-vertex set per
//!   path). This guarantees termination and matches the transitive-closure
//!   use case; Cypher's trail semantics (no repeated *edges*) is looser.
//! - Edge variable bindings on a ranged edge are rejected — Cypher binds a
//!   list of relationships there, which stage 1 does not model.

use crate::algo::{self, GraphNeighbor};
use crate::context::ExecutionContext;
use crate::error::{ExecutionError, Result};
use crate::match_impl::PatternMatcher;
use byoridb_parser::ast::{EdgeDirection, EdgePattern};
use futures::StreamExt;
use std::collections::HashSet;

/// Fetch one-hop neighbors honoring the pattern direction.
///
/// `Undirected` is the union of outgoing and incoming edges. A self-loop
/// occurs in both indexes and contributes one binding. Full edge identity
/// preserves parallel ranks and edges pointing in opposite directions.
pub(super) async fn neighbors_for_direction(
    ctx: &ExecutionContext,
    space: &str,
    vid: i64,
    edge_types: &[String],
    direction: &EdgeDirection,
) -> Result<Vec<GraphNeighbor>> {
    match direction {
        EdgeDirection::Outgoing => strict_neighbors(ctx, space, vid, edge_types, false).await,
        EdgeDirection::Incoming => strict_neighbors(ctx, space, vid, edge_types, true).await,
        EdgeDirection::Undirected => {
            let out = strict_neighbors(ctx, space, vid, edge_types, false).await?;
            let incoming = strict_neighbors(ctx, space, vid, edge_types, true).await?;
            let mut seen = HashSet::new();
            Ok(out
                .into_iter()
                .chain(incoming)
                .filter(|neighbor| {
                    let edge = &neighbor.edge;
                    seen.insert((
                        edge.src_vid,
                        edge.dst_vid,
                        edge.edge_type.clone(),
                        edge.ranking,
                    ))
                })
                .collect())
        }
    }
}

/// Read MATCH edge records fallibly. The general GO/recommendation helpers
/// retain their own contracts; only MATCH requires full key/payload validation.
async fn strict_neighbors(
    ctx: &ExecutionContext,
    space: &str,
    vid: i64,
    edge_types: &[String],
    incoming: bool,
) -> Result<Vec<GraphNeighbor>> {
    let kind = if incoming { "in-edge" } else { "edge" };
    let prefix = format!("{space}:{kind}:{vid}:");
    let mut stream = ctx.kvstore.scan_stream(prefix.as_bytes()).await?;
    let mut neighbors = Vec::new();
    while let Some(row) = stream.next().await {
        let (key, value) = row?;
        if !edge_types.is_empty()
            && algo::edge_type_from_key(&key)
                .is_some_and(|edge_type| !edge_types.iter().any(|wanted| wanted == edge_type))
        {
            continue;
        }
        let edge = algo::decode_query_edge(&key, &value)?;
        neighbors.push(GraphNeighbor {
            dst: if incoming { edge.src_vid } else { edge.dst_vid },
            edge,
        });
    }
    Ok(neighbors)
}

/// Expand a ranged edge pattern from `start_vid` and return the distinct
/// terminal vids reachable at a depth within `[min, max]`.
///
/// DFS with an explicit stack; each entry carries its own path (vertices
/// visited so far) for cycle prevention. `ctx.config.max_traversal_nodes`
/// caps the number of expansions. Needing an expansion beyond the cap fails
/// the query; partial terminal coverage must not become successful MATCH rows.
pub(super) async fn expand_var_length(
    ctx: &ExecutionContext,
    space: &str,
    start_vid: i64,
    edge: &EdgePattern,
    matcher: &PatternMatcher,
) -> Result<Vec<i64>> {
    let Some((min, max)) = edge.range else {
        return Err(ExecutionError::InvalidOperation(
            "expand_var_length called on a non-ranged edge pattern".to_string(),
        ));
    };

    if edge.variable.is_some() {
        return Err(ExecutionError::InvalidOperation(
            "variable binding on a variable-length edge (e.g. [e:t*1..3]) is not supported yet"
                .to_string(),
        ));
    }
    if max == 0 || min > max {
        return Err(ExecutionError::InvalidOperation(format!(
            "invalid variable-length range *{}..{}",
            min, max
        )));
    }
    let max_go_steps = ctx.config.max_go_steps as u64;
    if max_go_steps > 0 && max > max_go_steps {
        return Err(ExecutionError::InvalidOperation(format!(
            "variable-length range *{}..{} exceeds the maximum of {} steps",
            min, max, max_go_steps
        )));
    }

    let max_nodes = ctx.config.max_traversal_nodes as u64;
    let mut expansions: u64 = 0;
    let mut terminals: Vec<i64> = Vec::new();
    let mut terminal_set: HashSet<i64> = HashSet::new();

    // Stack entries: (vid, depth, path so far). Paths are short (≤ max ≤ 20),
    // so a linear `contains` beats cloning a HashSet per branch.
    let mut stack: Vec<(i64, u64, Vec<i64>)> = vec![(start_vid, 0, vec![start_vid])];

    while let Some((vid, depth, path)) = stack.pop() {
        if depth >= max {
            continue;
        }
        expansions += 1;
        if max_nodes > 0 && expansions > max_nodes {
            return Err(ExecutionError::ResourceExhausted(format!(
                "MATCH variable-length expansion exceeded max_traversal_nodes ({max_nodes}); query result is incomplete"
            )));
        }

        let neighbors =
            neighbors_for_direction(ctx, space, vid, &edge.edge_types, &edge.direction).await?;
        let mut next_vertices = Vec::with_capacity(neighbors.len());
        for neighbor in neighbors {
            if matcher.matches_edge_data(&neighbor.edge, edge)? && !path.contains(&neighbor.dst) {
                next_vertices.push(neighbor.dst);
            }
        }
        let keys: Vec<_> = next_vertices
            .iter()
            .map(|next| format!("{space}:vertex:{next}").into_bytes())
            .collect();
        let blobs = ctx.kvstore.batch_get(&keys).await?;
        for (next, blob) in next_vertices.into_iter().zip(blobs) {
            let Some(blob) = blob else {
                // Dangling edges cannot make an absent intermediate vertex
                // bridge a path to a later, present terminal.
                continue;
            };
            crate::record_decode::decode_vertex(next, &blob)?;
            let next_depth = depth + 1;
            if next_depth >= min && terminal_set.insert(next) {
                terminals.push(next);
            }
            if next_depth < max {
                let mut next_path = path.clone();
                next_path.push(next);
                stack.push((next, next_depth, next_path));
            }
        }
    }

    Ok(terminals)
}

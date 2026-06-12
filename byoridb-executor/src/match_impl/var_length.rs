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
use std::collections::HashSet;

/// Fetch one-hop neighbors honoring the pattern direction.
///
/// `Undirected` is the union of outgoing and incoming edges. The two scans
/// can never see the same stored edge twice (the forward prefix lists edges
/// *out of* `vid`, the in-edge prefix lists edges *into* it), so plain
/// concatenation is correct.
pub(super) async fn neighbors_for_direction(
    ctx: &ExecutionContext,
    space: &str,
    vid: i64,
    edge_types: &[String],
    direction: &EdgeDirection,
) -> Result<Vec<GraphNeighbor>> {
    match direction {
        EdgeDirection::Outgoing => algo::get_neighbors(ctx, space, vid, edge_types).await,
        EdgeDirection::Incoming => algo::get_incoming_neighbors(ctx, space, vid, edge_types).await,
        EdgeDirection::Undirected => {
            let mut out = algo::get_neighbors(ctx, space, vid, edge_types).await?;
            let incoming = algo::get_incoming_neighbors(ctx, space, vid, edge_types).await?;
            out.extend(incoming);
            Ok(out)
        }
    }
}

/// Expand a ranged edge pattern from `start_vid` and return the distinct
/// terminal vids reachable at a depth within `[min, max]`.
///
/// DFS with an explicit stack; each entry carries its own path (vertices
/// visited so far) for cycle prevention. `ctx.config.max_traversal_nodes`
/// caps the number of expansions — when hit, the result so far is returned
/// with a warning (terminals found are valid; coverage may be incomplete).
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
            tracing::warn!(
                max_traversal_nodes = max_nodes,
                "MATCH var-length expansion hit max_traversal_nodes; result may be incomplete"
            );
            break;
        }

        let neighbors =
            neighbors_for_direction(ctx, space, vid, &edge.edge_types, &edge.direction).await?;
        for neighbor in neighbors {
            if !matcher.matches_edge_data(&neighbor.edge, edge)? {
                continue;
            }
            let next = neighbor.dst;
            if path.contains(&next) {
                continue;
            }
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

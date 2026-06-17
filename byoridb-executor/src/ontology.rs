// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Shared ontology class-hierarchy helpers (PLAN.md O-3/O-7).
//!
//! Both the executor (class DDL, RECOMMEND) and the MATCH engine need to resolve
//! a vertex's class membership including transitive `SUBCLASS OF` ancestors.
//! These free functions take `&ExecutionContext` so any subsystem can call them
//! without going through `Executor`.

use crate::context::ExecutionContext;
use crate::error::{ExecutionError, Result};
use crate::key::SchemaKey;
use byoridb_codec::VertexCodec;
use serde::Deserialize;
use std::collections::HashSet;

/// Hard cap on hierarchy depth — guards corrupt metadata / cycles. Mirrors the
/// O-3 `class_ddl` creation-time cap.
const MAX_CLASS_DEPTH: usize = 16;

#[derive(Deserialize)]
struct ClassDef {
    superclasses: Vec<String>,
}

/// All transitive superclasses of class `name` (BFS, deduped, excludes `name`).
/// A plain tag with no class metadata yields an empty list. Errors if the
/// hierarchy exceeds [`MAX_CLASS_DEPTH`].
pub(crate) async fn class_ancestors_of(
    ctx: &ExecutionContext,
    space: &str,
    name: &str,
) -> Result<Vec<String>> {
    let mut ancestors: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::from([name.to_string()]);
    let mut frontier = vec![name.to_string()];

    for _depth in 0..MAX_CLASS_DEPTH {
        if frontier.is_empty() {
            return Ok(ancestors);
        }
        let mut next = Vec::new();
        for current in frontier {
            let Some(bytes) = ctx.kvstore.get(&SchemaKey::class(space, &current)).await? else {
                continue; // plain tag (no class hierarchy record)
            };
            let def: ClassDef = serde_json::from_slice(&bytes).map_err(|e| {
                ExecutionError::InvalidOperation(format!(
                    "Corrupt class metadata for {}: {}",
                    current, e
                ))
            })?;
            for parent in def.superclasses {
                if seen.insert(parent.clone()) {
                    ancestors.push(parent.clone());
                    next.push(parent);
                }
            }
        }
        frontier = next;
    }
    if frontier.is_empty() {
        Ok(ancestors)
    } else {
        Err(ExecutionError::InvalidOperation(format!(
            "class hierarchy of {} exceeds the maximum depth of {}",
            name, MAX_CLASS_DEPTH
        )))
    }
}

/// The set of classes a vertex belongs to: each of its tags plus their
/// transitive superclasses. `None` if the vertex does not exist. Used by
/// `is_a(...)` ontology filters in RECOMMEND and MATCH.
pub(crate) async fn vertex_class_set(
    ctx: &ExecutionContext,
    space: &str,
    vid: i64,
) -> Result<Option<HashSet<String>>> {
    let Some(blob) = ctx.kvstore.get(&SchemaKey::vertex(space, vid)).await? else {
        return Ok(None);
    };
    let vertex = VertexCodec::decode_vertex(&blob)
        .map_err(|e| ExecutionError::Io(std::io::Error::other(e.to_string())))?;
    let mut set = HashSet::new();
    for tag in &vertex.tags {
        set.insert(tag.name.clone());
        for ancestor in class_ancestors_of(ctx, space, &tag.name).await? {
            set.insert(ancestor);
        }
    }
    Ok(Some(set))
}

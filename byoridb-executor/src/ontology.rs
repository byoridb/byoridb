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

/// Hard cap on union-find chain length while resolving a representative —
/// guards corrupt pointers. With path compression at merge time the chain is
/// normally length 1, so this is purely defensive.
const MAX_SAMEAS_DEPTH: usize = 64;

/// Encode a representative vid for the `{space}:sameas:{vid}` pointer store.
/// 8-byte little-endian — the merge engine (`executor::sameas`) writes the same
/// format. Free function so both reader and writer share one encoding.
pub(crate) fn encode_repr(rep: i64) -> Vec<u8> {
    rep.to_le_bytes().to_vec()
}

fn decode_repr(bytes: &[u8]) -> Option<i64> {
    bytes.try_into().ok().map(i64::from_le_bytes)
}

/// Resolve a vertex's owl:sameAs canonical representative (PLAN.md O-8 D5).
/// Follows the `{space}:sameas:` union-find chain to the min-id representative;
/// returns `vid` unchanged when it has never been merged (no pointer, or a
/// self-pointer). Read-path helper called by GO/FETCH/MATCH to normalize input
/// vids onto the node that actually holds the merged facts.
pub(crate) async fn representative_of(
    ctx: &ExecutionContext,
    space: &str,
    vid: i64,
) -> Result<i64> {
    let mut cur = vid;
    for _ in 0..MAX_SAMEAS_DEPTH {
        let Some(bytes) = ctx.kvstore.get(&SchemaKey::sameas(space, cur)).await? else {
            return Ok(cur); // no pointer → own representative
        };
        let Some(rep) = decode_repr(&bytes) else {
            return Err(ExecutionError::InvalidOperation(format!(
                "Corrupt sameAs pointer for vertex {}",
                cur
            )));
        };
        if rep == cur {
            return Ok(cur); // self-pointer → representative
        }
        cur = rep;
    }
    Err(ExecutionError::InvalidOperation(format!(
        "sameAs chain for vertex {} exceeds depth {}",
        vid, MAX_SAMEAS_DEPTH
    )))
}

/// The non-representative vids collapsed into representative `rep`
/// (`{space}:sameas-members:{rep}:` scan). Empty when `rep` has no merged
/// members. Used by DELETE guards (O-8 D7) and introspection.
pub(crate) async fn members_of(ctx: &ExecutionContext, space: &str, rep: i64) -> Result<Vec<i64>> {
    let prefix = SchemaKey::sameas_members_prefix(space, rep);
    let mut members = Vec::new();
    for (key, _) in ctx.kvstore.scan_prefix(&prefix).await? {
        if let Some(m) = SchemaKey::sameas_member_from_key(&key) {
            members.push(m);
        }
    }
    Ok(members)
}

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

/// The set of classes a vertex belongs to: its tags **and** any inferred types
/// (O-5 domain/range, stored under `{space}:vtype:{vid}:`), each expanded with
/// their transitive superclasses. `None` if the vertex does not exist. Used by
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

    // Direct classes = declared tags ∪ inferred types.
    let mut direct: Vec<String> = vertex.tags.iter().map(|t| t.name.clone()).collect();
    let vtype_prefix = SchemaKey::vtype_prefix(space, vid);
    for (key, _) in ctx.kvstore.scan_prefix(&vtype_prefix).await? {
        if let Some(class) = SchemaKey::vtype_class_from_key(&key) {
            direct.push(class);
        }
    }

    let mut set = HashSet::new();
    for class in direct {
        if set.insert(class.clone()) {
            for ancestor in class_ancestors_of(ctx, space, &class).await? {
                set.insert(ancestor);
            }
        }
    }
    Ok(Some(set))
}

// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! FIXED_STRING VID compatibility layer.
//!
//! The established storage/codec/RPC contract uses i64 VIDs. String-VID spaces
//! therefore persist a per-space, bidirectional mapping and route the stable
//! internal surrogate through those existing paths. User-facing query results
//! convert the surrogate back to the original string.

use crate::context::ExecutionContext;
use crate::error::{ExecutionError, Result};
use crate::key::SchemaKey;
use crate::plan::Vid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpaceVidType {
    Int64,
    FixedString(usize),
}

pub(crate) async fn space_vid_type(ctx: &ExecutionContext, space: &str) -> Result<SpaceVidType> {
    let Some(bytes) = ctx.kvstore.get(&SchemaKey::space(space)).await? else {
        // Unit-level/local embedders historically constructed an execution
        // context with a selected space but no persisted space descriptor.
        // Preserve that established INT64-only contract; real graph-service
        // sessions validate USE against persisted metadata before execution.
        return Ok(SpaceVidType::Int64);
    };
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    let raw = value
        .get("vid_type")
        .and_then(|v| v.as_str())
        .unwrap_or("INT64");
    if raw.eq_ignore_ascii_case("INT64") {
        return Ok(SpaceVidType::Int64);
    }
    let upper = raw.to_ascii_uppercase();
    if let Some(length) = upper
        .strip_prefix("FIXED_STRING(")
        .and_then(|s| s.strip_suffix(')'))
        .and_then(|s| s.parse::<usize>().ok())
    {
        return fixed_string_vid_type(space, length, ctx.is_distributed());
    }
    Err(ExecutionError::InvalidOperation(format!(
        "space '{space}' has unsupported vid_type '{raw}'"
    )))
}

fn fixed_string_vid_type(space: &str, length: usize, distributed: bool) -> Result<SpaceVidType> {
    if distributed {
        return Err(ExecutionError::InvalidOperation(format!(
            "space '{space}' uses FIXED_STRING VIDs, which are unsupported in distributed execution until mapping ownership and replication are implemented; use INT64"
        )));
    }
    Ok(SpaceVidType::FixedString(length))
}

fn encode_component(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(value.len() * 2);
    for byte in value.as_bytes() {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn forward_key(space: &str, value: &str) -> Vec<u8> {
    format!("{space}:vid-map:{}", encode_component(value)).into_bytes()
}

fn reverse_key(space: &str, internal: i64) -> Vec<u8> {
    format!("{space}:vid-rev:{internal}").into_bytes()
}

fn decode_internal(bytes: &[u8]) -> Option<i64> {
    bytes.try_into().ok().map(i64::from_be_bytes)
}

/// New string mappings live exclusively in the negative half of i64. The
/// non-negative half remains disjoint for the temporary legacy read/delete
/// bridge used by spaces that predate persisted string mappings.
fn initial_candidate(value: &str) -> i64 {
    (byoridb_common::hash::hash_bytes(value.as_bytes()) | (1_u64 << 63)) as i64
}

/// Probe every negative i64, wrapping from -1 to i64::MIN without ever
/// entering the non-negative legacy namespace.
fn next_candidate(candidate: i64) -> i64 {
    debug_assert!(candidate < 0);
    if candidate == -1 {
        i64::MIN
    } else {
        candidate + 1
    }
}

/// A surrogate cannot claim a number already present in graph data, even when
/// mapping metadata is absent. Check a vertex plus both endpoint-oriented edge
/// keyspaces before atomically claiming the reverse mapping.
async fn internal_vid_is_occupied(
    ctx: &ExecutionContext,
    space: &str,
    internal: i64,
) -> Result<bool> {
    if ctx
        .kvstore
        .get(&SchemaKey::vertex(space, internal))
        .await?
        .is_some()
    {
        return Ok(true);
    }
    if !ctx
        .kvstore
        .scan_prefix_limited(&SchemaKey::edge_data_src_prefix(space, internal), Some(1))
        .await?
        .is_empty()
    {
        return Ok(true);
    }
    Ok(!ctx
        .kvstore
        .scan_prefix_limited(
            &SchemaKey::in_edge_data_dst_prefix(space, internal),
            Some(1),
        )
        .await?
        .is_empty())
}

pub(crate) fn validate_write_vid(space: &str, vid_type: SpaceVidType, vid: &Vid) -> Result<()> {
    match (vid_type, vid) {
        (SpaceVidType::Int64, Vid::Int(_)) => Ok(()),
        (SpaceVidType::Int64, Vid::String(_)) => Err(ExecutionError::InvalidOperation(format!(
            "space '{space}' uses INT64 VIDs; a string VID is not allowed"
        ))),
        (SpaceVidType::FixedString(max_len), Vid::String(value)) => {
            let actual_len = value.len();
            if actual_len > max_len {
                Err(ExecutionError::InvalidOperation(format!(
                    "VID is {actual_len} bytes but space '{space}' uses FIXED_STRING({max_len})"
                )))
            } else {
                Ok(())
            }
        }
        (SpaceVidType::FixedString(_), Vid::Int(_)) => Err(ExecutionError::InvalidOperation(
            format!(
                "space '{space}' uses FIXED_STRING VIDs; integer VID {vid} is read/delete-only legacy data and cannot be written"
            ),
        )),
    }
}

/// Resolve one user VID. `create_mapping` is true for INSERT and UPDATE-upsert.
/// Unknown strings on read/delete paths return `None` and never probe or
/// materialize mapping metadata.
pub(crate) async fn resolve_vid(
    ctx: &ExecutionContext,
    space: &str,
    vid_type: SpaceVidType,
    vid: &Vid,
    create_mapping: bool,
) -> Result<Option<i64>> {
    match (vid_type, vid) {
        (SpaceVidType::Int64, Vid::Int(value)) => Ok(Some(*value)),
        (SpaceVidType::Int64, Vid::String(_)) => Err(ExecutionError::InvalidOperation(format!(
            "space '{space}' uses INT64 VIDs; a string VID is not allowed"
        ))),
        (SpaceVidType::FixedString(_), Vid::Int(_)) if create_mapping => {
            Err(ExecutionError::InvalidOperation(format!(
                "space '{space}' uses FIXED_STRING VIDs; integer VID {vid} is read/delete-only legacy data and cannot be written"
            )))
        }
        (SpaceVidType::FixedString(_), Vid::Int(value)) if *value < 0 => {
            Err(ExecutionError::InvalidOperation(format!(
                "space '{space}' uses FIXED_STRING VIDs; raw negative internal VID {value} is not allowed"
            )))
        }
        (SpaceVidType::FixedString(_), Vid::Int(value)) => {
            if ctx.kvstore.get(&reverse_key(space, *value)).await?.is_some() {
                return Err(ExecutionError::InvalidOperation(format!(
                    "corrupt positive string VID mapping for internal VID {value} in space '{space}'"
                )));
            }
            if internal_vid_is_occupied(ctx, space, *value).await? {
                ctx.warn_legacy_fixed_string_vid(space, *value);
                Ok(Some(*value))
            } else {
                Ok(None)
            }
        }
        (SpaceVidType::FixedString(max_len), Vid::String(value)) => {
            let actual_len = value.len();
            if actual_len > max_len {
                return Err(ExecutionError::InvalidOperation(format!(
                    "VID is {actual_len} bytes but space '{space}' uses FIXED_STRING({max_len})"
                )));
            }
            let fwd = forward_key(space, value);
            if let Some(bytes) = ctx.kvstore.get(&fwd).await? {
                let internal = decode_internal(&bytes).ok_or_else(|| {
                    ExecutionError::InvalidOperation(format!(
                        "corrupt string VID mapping in space '{space}'"
                    ))
                })?;
                if internal >= 0 {
                    return Err(ExecutionError::InvalidOperation(format!(
                        "string VID mapping in space '{space}' points outside the negative surrogate namespace"
                    )));
                }
                let reverse = ctx
                    .kvstore
                    .get(&reverse_key(space, internal))
                    .await?
                    .ok_or_else(|| {
                        ExecutionError::InvalidOperation(format!(
                            "missing reverse string VID mapping for internal VID {internal} in space '{space}'"
                        ))
                    })?;
                if reverse != value.as_bytes() {
                    return Err(ExecutionError::InvalidOperation(format!(
                        "conflicting reverse string VID mapping for internal VID {internal} in space '{space}'"
                    )));
                }
                return Ok(Some(internal));
            }
            if !create_mapping {
                return Ok(None);
            }
            // Deterministic initial placement keeps repeated concurrent inserts
            // convergent. Probe only when an existing reverse mapping proves a
            // hash collision; mappings are never recycled after DELETE.
            let first_candidate = initial_candidate(value);
            let mut candidate = first_candidate;
            loop {
                let rev = reverse_key(space, candidate);
                let existing_reverse = ctx.kvstore.get(&rev).await?;
                let reverse_owner = match existing_reverse {
                    Some(existing) => Some(existing),
                    None => {
                        if internal_vid_is_occupied(ctx, space, candidate).await? {
                            candidate = next_candidate(candidate);
                            if candidate == first_candidate {
                                return Err(ExecutionError::InvalidOperation(format!(
                                    "negative string VID namespace is exhausted in space '{space}'"
                                )));
                            }
                            continue;
                        }
                        // The reverse key is the uniqueness claim for an
                        // internal surrogate. Claim it atomically before
                        // publishing the forward mapping.
                        ctx.kvstore.put_if_absent(&rev, value.as_bytes()).await?
                    }
                };
                match reverse_owner {
                    Some(existing) if existing == value.as_bytes() => {
                        let encoded = candidate.to_be_bytes();
                        if let Some(existing) = ctx.kvstore.put_if_absent(&fwd, &encoded).await? {
                            if decode_internal(&existing) != Some(candidate) {
                                return Err(ExecutionError::InvalidOperation(format!(
                                    "conflicting string VID mapping in space '{space}'"
                                )));
                            }
                        }
                        return Ok(Some(candidate));
                    }
                    Some(_) => {
                        candidate = next_candidate(candidate);
                        if candidate == first_candidate {
                            return Err(ExecutionError::InvalidOperation(format!(
                                "negative string VID namespace is exhausted in space '{space}'"
                            )));
                        }
                    }
                    None => {
                        // `None` from put_if_absent means this call now owns the
                        // reverse key. Publishing the forward key separately is
                        // crash-safe: a retry recognizes the reverse owner and
                        // repairs a missing forward entry. Never overwrite a
                        // conflicting forward entry.
                        let encoded = candidate.to_be_bytes();
                        if let Some(existing) = ctx.kvstore.put_if_absent(&fwd, &encoded).await? {
                            if decode_internal(&existing) != Some(candidate) {
                                return Err(ExecutionError::InvalidOperation(format!(
                                    "conflicting string VID mapping in space '{space}'"
                                )));
                            }
                        }
                        return Ok(Some(candidate));
                    }
                }
            }
        }
    }
}

pub(crate) async fn display_vid(
    ctx: &ExecutionContext,
    space: &str,
    vid_type: SpaceVidType,
    internal: i64,
) -> Result<byoridb_common::Value> {
    match vid_type {
        SpaceVidType::Int64 => Ok(byoridb_common::Value::Int(internal)),
        SpaceVidType::FixedString(_) => {
            if internal >= 0 {
                if ctx
                    .kvstore
                    .get(&reverse_key(space, internal))
                    .await?
                    .is_some()
                {
                    return Err(ExecutionError::InvalidOperation(format!(
                        "corrupt positive string VID mapping for internal VID {internal} in space '{space}'"
                    )));
                }
                if !internal_vid_is_occupied(ctx, space, internal).await? {
                    return Err(ExecutionError::InvalidOperation(format!(
                        "unmapped non-negative VID {internal} is not live in FIXED_STRING space '{space}'"
                    )));
                }
                ctx.warn_legacy_fixed_string_vid(space, internal);
                return Ok(byoridb_common::Value::Int(internal));
            }
            let value = ctx
                .kvstore
                .get(&reverse_key(space, internal))
                .await?
                .ok_or_else(|| {
                    ExecutionError::InvalidOperation(format!(
                        "missing reverse string VID mapping for internal VID {internal} in space '{space}'"
                    ))
                })?;
            let value = String::from_utf8(value).map_err(|_| {
                ExecutionError::InvalidOperation(format!(
                    "invalid UTF-8 reverse string VID mapping in space '{space}'"
                ))
            })?;
            Ok(byoridb_common::Value::String(value))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use byoridb_kvstore::store::MemoryKVStore;
    use std::sync::Arc;

    async fn fixed_context(max_len: usize) -> ExecutionContext {
        let ctx = ExecutionContext::new(Arc::new(MemoryKVStore::new()))
            .with_space("accounts".to_string());
        let metadata = serde_json::json!({
            "id": 7,
            "name": "accounts",
            "vid_type": format!("FIXED_STRING({max_len})"),
        });
        ctx.kvstore
            .put(
                &SchemaKey::space("accounts"),
                &serde_json::to_vec(&metadata).unwrap(),
            )
            .await
            .unwrap();
        ctx
    }

    #[tokio::test]
    async fn fixed_string_mapping_is_persistent_and_bidirectional() {
        let ctx = fixed_context(32).await;
        let vid_type = space_vid_type(&ctx, "accounts").await.unwrap();
        let external = Vid::String("acct-001".to_string());

        let internal = resolve_vid(&ctx, "accounts", vid_type, &external, true)
            .await
            .unwrap()
            .unwrap();
        assert!(internal < 0);
        assert_eq!(
            ctx.kvstore
                .get(&forward_key("accounts", "acct-001"))
                .await
                .unwrap(),
            Some(internal.to_be_bytes().to_vec())
        );
        assert_eq!(
            ctx.kvstore
                .get(&reverse_key("accounts", internal))
                .await
                .unwrap(),
            Some(b"acct-001".to_vec())
        );
        assert_eq!(
            resolve_vid(&ctx, "accounts", vid_type, &external, false)
                .await
                .unwrap(),
            Some(internal)
        );
        assert_eq!(
            display_vid(&ctx, "accounts", vid_type, internal)
                .await
                .unwrap(),
            byoridb_common::Value::String("acct-001".to_string())
        );
    }

    #[tokio::test]
    async fn unknown_read_does_not_materialize_mapping_and_length_is_utf8_bytes() {
        let ctx = fixed_context(4).await;
        let vid_type = space_vid_type(&ctx, "accounts").await.unwrap();
        let unknown = Vid::String("none".to_string());

        assert_eq!(
            resolve_vid(&ctx, "accounts", vid_type, &unknown, false)
                .await
                .unwrap(),
            None
        );
        assert!(ctx
            .kvstore
            .get(&forward_key("accounts", "none"))
            .await
            .unwrap()
            .is_none());

        let too_long = Vid::String("한글".to_string());
        let error = resolve_vid(&ctx, "accounts", vid_type, &too_long, true)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("6 bytes"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_hash_collisions_reserve_distinct_internal_vids() {
        // hash_bytes zero-pads its final chunk, so these two valid UTF-8 VIDs
        // intentionally collide and exercise the probing path without relying
        // on a probabilistic hash collision.
        let first = "a";
        let second = "a\0";
        assert_eq!(
            byoridb_common::hash::hash_bytes(first.as_bytes()) & i64::MAX as u64,
            byoridb_common::hash::hash_bytes(second.as_bytes()) & i64::MAX as u64
        );

        let ctx = Arc::new(fixed_context(8).await);
        let vid_type = space_vid_type(&ctx, "accounts").await.unwrap();
        let barrier = Arc::new(tokio::sync::Barrier::new(2));

        let resolve = |external: &'static str| {
            let ctx = Arc::clone(&ctx);
            let barrier = Arc::clone(&barrier);
            tokio::spawn(async move {
                barrier.wait().await;
                resolve_vid(
                    &ctx,
                    "accounts",
                    vid_type,
                    &Vid::String(external.to_string()),
                    true,
                )
                .await
                .unwrap()
                .unwrap()
            })
        };

        let (first_internal, second_internal) = tokio::join!(resolve(first), resolve(second));
        let first_internal = first_internal.unwrap();
        let second_internal = second_internal.unwrap();
        assert_ne!(first_internal, second_internal);
        assert!(first_internal < 0);
        assert!(second_internal < 0);
        assert_eq!(
            display_vid(&ctx, "accounts", vid_type, first_internal)
                .await
                .unwrap(),
            byoridb_common::Value::String(first.to_string())
        );
        assert_eq!(
            display_vid(&ctx, "accounts", vid_type, second_internal)
                .await
                .unwrap(),
            byoridb_common::Value::String(second.to_string())
        );
    }

    #[test]
    fn negative_probe_wraps_without_entering_legacy_namespace() {
        assert_eq!(next_candidate(-2), -1);
        assert_eq!(next_candidate(-1), i64::MIN);
        assert_eq!(next_candidate(i64::MIN), i64::MIN + 1);
    }

    #[test]
    fn distributed_fixed_string_mapping_is_rejected() {
        let error = fixed_string_vid_type("accounts", 32, true).unwrap_err();
        assert!(error
            .to_string()
            .contains("unsupported in distributed execution"));
        assert_eq!(
            fixed_string_vid_type("accounts", 32, false).unwrap(),
            SpaceVidType::FixedString(32)
        );
    }

    #[tokio::test]
    async fn mapping_skips_occupied_negative_vertex_and_edge_endpoints() {
        let ctx = fixed_context(32).await;
        let vid_type = space_vid_type(&ctx, "accounts").await.unwrap();
        let external = "occupied-chain";
        let vertex_candidate = initial_candidate(external);
        let outgoing_candidate = next_candidate(vertex_candidate);
        let incoming_candidate = next_candidate(outgoing_candidate);
        let expected = next_candidate(incoming_candidate);

        ctx.kvstore
            .put(
                &SchemaKey::vertex("accounts", vertex_candidate),
                b"occupied",
            )
            .await
            .unwrap();
        ctx.kvstore
            .put(
                &SchemaKey::edge_data("accounts", outgoing_candidate, "owns", 1, 0),
                b"occupied",
            )
            .await
            .unwrap();
        ctx.kvstore
            .put(
                &SchemaKey::in_edge_data("accounts", incoming_candidate, "owns", 1, 0),
                b"occupied",
            )
            .await
            .unwrap();

        let internal = resolve_vid(
            &ctx,
            "accounts",
            vid_type,
            &Vid::String(external.to_string()),
            true,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(internal, expected);
        assert!(internal < 0);
    }

    #[tokio::test]
    async fn fixed_string_legacy_positive_vid_is_read_delete_only() {
        let ctx = fixed_context(32).await;
        let vid_type = space_vid_type(&ctx, "accounts").await.unwrap();
        let legacy = Vid::Int(42);
        ctx.kvstore
            .put(&SchemaKey::vertex("accounts", 42), b"legacy")
            .await
            .unwrap();

        assert_eq!(
            resolve_vid(&ctx, "accounts", vid_type, &legacy, false)
                .await
                .unwrap(),
            Some(42)
        );
        assert_eq!(
            display_vid(&ctx, "accounts", vid_type, 42).await.unwrap(),
            byoridb_common::Value::Int(42)
        );
        assert!(resolve_vid(&ctx, "accounts", vid_type, &legacy, true)
            .await
            .is_err());
        assert!(
            resolve_vid(&ctx, "accounts", vid_type, &Vid::Int(-42), false)
                .await
                .is_err()
        );
        assert_eq!(
            resolve_vid(&ctx, "accounts", vid_type, &Vid::Int(404), false)
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn corrupt_mapping_namespace_and_missing_reverse_are_errors() {
        let ctx = fixed_context(32).await;
        let vid_type = space_vid_type(&ctx, "accounts").await.unwrap();
        ctx.kvstore
            .put(&forward_key("accounts", "positive"), &7_i64.to_be_bytes())
            .await
            .unwrap();
        let error = resolve_vid(
            &ctx,
            "accounts",
            vid_type,
            &Vid::String("positive".to_string()),
            false,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("negative surrogate namespace"));

        ctx.kvstore
            .put(&forward_key("accounts", "alice"), &(-9_i64).to_be_bytes())
            .await
            .unwrap();
        ctx.kvstore
            .put(&reverse_key("accounts", -9), b"bob")
            .await
            .unwrap();
        let error = resolve_vid(
            &ctx,
            "accounts",
            vid_type,
            &Vid::String("alice".to_string()),
            false,
        )
        .await
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("conflicting reverse string VID mapping"));

        ctx.kvstore
            .put(&SchemaKey::vertex("accounts", -7), b"corrupt")
            .await
            .unwrap();
        let error = display_vid(&ctx, "accounts", vid_type, -7)
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("missing reverse string VID mapping"));
    }
}

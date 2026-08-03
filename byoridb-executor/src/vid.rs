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
        return Ok(SpaceVidType::FixedString(length));
    }
    Err(ExecutionError::InvalidOperation(format!(
        "space '{space}' has unsupported vid_type '{raw}'"
    )))
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

/// Resolve one user VID. `create_mapping` is true for INSERT and UPDATE-upsert.
/// Read/delete/traversal callers receive a non-materialized stable surrogate
/// for an unknown string, which follows the normal point-miss path.
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
        (SpaceVidType::FixedString(_), Vid::Int(_)) => Err(ExecutionError::InvalidOperation(
            format!("space '{space}' uses FIXED_STRING VIDs; integer VID {vid} is not allowed"),
        )),
        (SpaceVidType::FixedString(max_len), Vid::String(value)) => {
            let actual_len = value.len();
            if actual_len > max_len {
                return Err(ExecutionError::InvalidOperation(format!(
                    "VID is {actual_len} bytes but space '{space}' uses FIXED_STRING({max_len})"
                )));
            }
            let fwd = forward_key(space, value);
            if let Some(bytes) = ctx.kvstore.get(&fwd).await? {
                return decode_internal(&bytes).map(Some).ok_or_else(|| {
                    ExecutionError::InvalidOperation(format!(
                        "corrupt string VID mapping in space '{space}'"
                    ))
                });
            }
            // Deterministic initial placement keeps repeated concurrent inserts
            // convergent. Probe only when an existing reverse mapping proves a
            // hash collision; mappings are never recycled after DELETE.
            let mut candidate =
                (byoridb_common::hash::hash_bytes(value.as_bytes()) & i64::MAX as u64) as i64;
            if candidate == 0 {
                candidate = 1;
            }
            loop {
                let rev = reverse_key(space, candidate);
                let reverse_owner = if create_mapping {
                    // The reverse key is the uniqueness claim for an internal
                    // surrogate. A plain get followed by batch_put lets two
                    // colliding strings both observe an empty key and alias the
                    // same vertex. Claim it atomically before publishing the
                    // forward mapping.
                    ctx.kvstore.put_if_absent(&rev, value.as_bytes()).await?
                } else {
                    ctx.kvstore.get(&rev).await?
                };
                match reverse_owner {
                    Some(existing) if existing == value.as_bytes() => {
                        if create_mapping {
                            let encoded = candidate.to_be_bytes();
                            if let Some(existing) =
                                ctx.kvstore.put_if_absent(&fwd, &encoded).await?
                            {
                                if decode_internal(&existing) != Some(candidate) {
                                    return Err(ExecutionError::InvalidOperation(format!(
                                        "conflicting string VID mapping in space '{space}'"
                                    )));
                                }
                            }
                        }
                        return Ok(Some(candidate));
                    }
                    Some(_) => {
                        candidate = if candidate == i64::MAX {
                            1
                        } else {
                            candidate + 1
                        };
                    }
                    None => {
                        if !create_mapping {
                            // A stable non-materialized surrogate makes unknown
                            // string IDs follow normal point-miss semantics in
                            // FETCH/GO/FIND/MATCH without creating metadata.
                            return Ok(Some(candidate));
                        }
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
        assert!(internal > 0);
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

        assert!(resolve_vid(&ctx, "accounts", vid_type, &unknown, false)
            .await
            .unwrap()
            .is_some());
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
}

// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Strict vertex validation for query paths. Missing legacy JSON fields retain
//! their documented defaults; malformed supplied fields must not become no-match.

use crate::error::{ExecutionError, Result};
use byoridb_codec::{VertexCodec, VertexData};

fn invalid_vertex() -> ExecutionError {
    byoridb_storage::StorageError::DecodingError("Invalid vertex metadata".to_string()).into()
}

fn validate_json(expected_vid: i64, vertex: &serde_json::Value) -> Result<()> {
    if vertex
        .get("vid")
        .is_some_and(|vid| vid.as_i64() != Some(expected_vid))
    {
        return Err(invalid_vertex());
    }
    let tags = vertex
        .get("tags")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(invalid_vertex)?;
    if tags.iter().any(|tag| {
        tag.get("name")
            .and_then(serde_json::Value::as_str)
            .is_none()
            || tag.get("props").is_some_and(|props| !props.is_object())
    }) {
        return Err(invalid_vertex());
    }
    Ok(())
}

/// Decode a vertex fetched using an authoritative key/VID, including legacy
/// JSON that omits its VID or tag properties. Explicit identities must match.
pub(crate) fn decode_vertex(expected_vid: i64, data: &[u8]) -> Result<VertexData> {
    let proto = VertexCodec::is_proto_format(data);
    if !proto {
        let json = serde_json::from_slice(data)?;
        validate_json(expected_vid, &json)?;
    }
    let mut vertex =
        VertexCodec::decode_vertex(data).map_err(byoridb_storage::StorageError::from)?;
    if proto && vertex.vid != expected_vid {
        return Err(invalid_vertex());
    }
    // The legacy codec defaults an absent JSON VID to zero. The storage key
    // supplies its identity; never substitute zero for a different stored VID.
    vertex.vid = expected_vid;
    Ok(vertex)
}

/// Preserve LOOKUP's legacy JSON result representation after validating it.
pub(crate) fn decode_vertex_json(expected_vid: i64, data: &[u8]) -> Result<serde_json::Value> {
    if VertexCodec::is_proto_format(data) {
        return Ok(VertexCodec::vertex_to_json(&decode_vertex(
            expected_vid,
            data,
        )?));
    }
    let json = serde_json::from_slice(data)?;
    validate_json(expected_vid, &json)?;
    Ok(json)
}

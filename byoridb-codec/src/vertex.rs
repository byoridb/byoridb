// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Vertex and Edge encoding/decoding with Protocol Buffers
//!
//! This module provides efficient serialization for graph data using Protocol Buffers,
//! with JSON fallback for backward compatibility.
//!
//! # Format
//! - Proto format: ~300 bytes/record (70% smaller than JSON)
//! - JSON format: ~1KB/record (legacy, for backward compatibility)
//!
//! # Usage
//! ```ignore
//! use byoridb_codec::vertex::{VertexCodec, VertexData, TagData};
//!
//! // Encode a vertex
//! let vertex = VertexData {
//!     vid: 123,
//!     tags: vec![TagData {
//!         name: "person".to_string(),
//!         properties: [("name".to_string(), Value::String("Alice".to_string()))].into(),
//!     }],
//! };
//! let bytes = VertexCodec::encode(&vertex)?;
//!
//! // Decode a vertex
//! let decoded = VertexCodec::decode(&bytes)?;
//! ```

use crate::error::{CodecError, Result};
use crate::proto::data as pb;
use byoridb_common::datatypes::list::List as CommonList;
use byoridb_common::datatypes::map::Map as CommonMap;
use prost::Message;
use std::collections::HashMap;

/// Magic byte to identify Proto-encoded data
const PROTO_MAGIC: u8 = 0xCA; // 'CA' for ByoriDB

/// Minimal protobuf varint reader for fast-path decoders.
/// Returns `(value, bytes_consumed)` or `None` if the buffer is truncated.
fn read_varint(buf: &[u8]) -> Option<(u64, usize)> {
    let mut result: u64 = 0;
    let mut shift = 0;
    for (i, &byte) in buf.iter().enumerate().take(10) {
        result |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some((result, i + 1));
        }
        shift += 7;
    }
    None
}

/// Vertex data structure
#[derive(Debug, Clone, PartialEq)]
pub struct VertexData {
    pub vid: i64,
    pub tags: Vec<TagData>,
}

/// Tag data within a vertex
#[derive(Debug, Clone, PartialEq)]
pub struct TagData {
    pub name: String,
    pub properties: HashMap<String, byoridb_common::Value>,
}

/// Edge data structure
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeData {
    pub src_vid: i64,
    pub dst_vid: i64,
    pub edge_type: String,
    pub ranking: i64,
    pub properties: HashMap<String, byoridb_common::Value>,
}

/// Vertex/Edge codec with Proto encoding and JSON fallback
pub struct VertexCodec;

impl VertexCodec {
    /// Encode vertex data to bytes (Proto format)
    pub fn encode_vertex(vertex: &VertexData) -> Result<Vec<u8>> {
        let proto_vertex = Self::vertex_to_proto(vertex);
        let mut buf = Vec::with_capacity(proto_vertex.encoded_len() + 1);
        buf.push(PROTO_MAGIC);
        proto_vertex
            .encode(&mut buf)
            .map_err(|e| CodecError::IncorrectValue(format!("Proto encode failed: {}", e)))?;
        Ok(buf)
    }

    /// Decode vertex data from bytes (auto-detect format)
    pub fn decode_vertex(data: &[u8]) -> Result<VertexData> {
        if data.is_empty() {
            return Err(CodecError::IncorrectValue("Empty data".to_string()));
        }

        if data[0] == PROTO_MAGIC {
            // Proto format
            Self::decode_vertex_proto(&data[1..])
        } else {
            // JSON fallback
            Self::decode_vertex_json(data)
        }
    }

    /// Encode edge data to bytes (Proto format)
    pub fn encode_edge(edge: &EdgeData) -> Result<Vec<u8>> {
        let proto_edge = Self::edge_to_proto(edge);
        let mut buf = Vec::with_capacity(proto_edge.encoded_len() + 1);
        buf.push(PROTO_MAGIC);
        proto_edge
            .encode(&mut buf)
            .map_err(|e| CodecError::IncorrectValue(format!("Proto encode failed: {}", e)))?;
        Ok(buf)
    }

    /// Decode edge data from bytes (auto-detect format)
    pub fn decode_edge(data: &[u8]) -> Result<EdgeData> {
        if data.is_empty() {
            return Err(CodecError::IncorrectValue("Empty data".to_string()));
        }

        if data[0] == PROTO_MAGIC {
            // Proto format
            Self::decode_edge_proto(&data[1..])
        } else {
            // JSON fallback
            Self::decode_edge_json(data)
        }
    }

    /// Check if data is in Proto format
    pub fn is_proto_format(data: &[u8]) -> bool {
        !data.is_empty() && data[0] == PROTO_MAGIC
    }

    /// Fast-path decoder that returns only `dst_vid` without materializing
    /// the rest of [`EdgeData`].
    ///
    /// BFS / `GO 1 STEP` / outgoing-only traversals don't need edge type,
    /// ranking, or properties — full proto decode allocates a `HashMap` per
    /// edge to hold properties even when they're unused. This walks the
    /// proto field stream and stops at field 2.
    pub fn decode_edge_dst(data: &[u8]) -> Result<i64> {
        if data.is_empty() {
            return Err(CodecError::IncorrectValue("Empty data".to_string()));
        }

        if data[0] == PROTO_MAGIC {
            Self::decode_edge_vid_proto(&data[1..], 2)
        } else {
            // JSON fallback — small overhead, but JSON-encoded edges are
            // expected to be the legacy path so we accept it.
            let json: serde_json::Value = serde_json::from_slice(data)
                .map_err(|e| CodecError::IncorrectValue(format!("JSON decode failed: {}", e)))?;
            Ok(json.get("dst").and_then(|v| v.as_i64()).unwrap_or_default())
        }
    }

    /// Fast-path decoder that returns only `src_vid` (proto field 1).
    ///
    /// Counterpart of [`decode_edge_dst`](Self::decode_edge_dst) for reverse
    /// traversal over the in-edge index, whose values are denormalized edge
    /// payloads: the neighbor a BFS expands to is the edge's *source*.
    pub fn decode_edge_src(data: &[u8]) -> Result<i64> {
        if data.is_empty() {
            return Err(CodecError::IncorrectValue("Empty data".to_string()));
        }

        if data[0] == PROTO_MAGIC {
            Self::decode_edge_vid_proto(&data[1..], 1)
        } else {
            let json: serde_json::Value = serde_json::from_slice(data)
                .map_err(|e| CodecError::IncorrectValue(format!("JSON decode failed: {}", e)))?;
            Ok(json.get("src").and_then(|v| v.as_i64()).unwrap_or_default())
        }
    }

    fn decode_edge_vid_proto(data: &[u8], target_field: u64) -> Result<i64> {
        // proto field tag = (field_number << 3) | wire_type. Wire types we
        // care about: 0=varint, 2=length-delimited.
        let mut pos = 0;
        while pos < data.len() {
            let (tag, n) = read_varint(&data[pos..])
                .ok_or_else(|| CodecError::IncorrectValue("Truncated tag".to_string()))?;
            pos += n;
            let field_num = tag >> 3;
            let wire_type = (tag & 0x7) as u8;
            if field_num == target_field && wire_type == 0 {
                let (val, _) = read_varint(&data[pos..])
                    .ok_or_else(|| CodecError::IncorrectValue("Truncated vid field".to_string()))?;
                return Ok(val as i64);
            }
            // Skip this field.
            match wire_type {
                0 => {
                    let (_, n) = read_varint(&data[pos..]).ok_or_else(|| {
                        CodecError::IncorrectValue("Truncated varint".to_string())
                    })?;
                    pos += n;
                }
                1 => pos += 8,
                2 => {
                    let (len, n) = read_varint(&data[pos..]).ok_or_else(|| {
                        CodecError::IncorrectValue("Truncated length prefix".to_string())
                    })?;
                    pos += n + len as usize;
                }
                5 => pos += 4,
                _ => {
                    return Err(CodecError::IncorrectValue(format!(
                        "Unsupported wire type {}",
                        wire_type
                    )))
                }
            }
        }
        // The vid is proto-default (0) when omitted, which is also our
        // EdgeData default. Match decode_edge_proto's semantics.
        Ok(0)
    }

    // === Proto encoding/decoding ===

    fn vertex_to_proto(vertex: &VertexData) -> pb::Vertex {
        pb::Vertex {
            vid: vertex.vid,
            tags: vertex.tags.iter().map(Self::tag_to_proto).collect(),
        }
    }

    fn tag_to_proto(tag: &TagData) -> pb::Tag {
        pb::Tag {
            name: tag.name.clone(),
            properties: tag
                .properties
                .iter()
                .map(|(k, v)| (k.clone(), Self::value_to_proto(v)))
                .collect(),
        }
    }

    fn edge_to_proto(edge: &EdgeData) -> pb::Edge {
        pb::Edge {
            src_vid: edge.src_vid,
            dst_vid: edge.dst_vid,
            edge_type: edge.edge_type.clone(),
            ranking: edge.ranking,
            properties: edge
                .properties
                .iter()
                .map(|(k, v)| (k.clone(), Self::value_to_proto(v)))
                .collect(),
        }
    }

    fn value_to_proto(value: &byoridb_common::Value) -> pb::Value {
        use byoridb_common::Value as V;

        let inner = match value {
            V::Null(_) => Some(pb::value::Value::NullValue(0)),
            V::Bool(b) => Some(pb::value::Value::BoolValue(*b)),
            V::Int(i) => Some(pb::value::Value::IntValue(*i)),
            V::Float(f) => Some(pb::value::Value::FloatValue(*f)),
            V::String(s) => Some(pb::value::Value::StringValue(s.clone())),
            V::Date(d) => Some(pb::value::Value::DateValue(pb::Date {
                year: d.year as u32,
                month: d.month as u32,
                day: d.day as u32,
            })),
            V::DateTime(dt) => Some(pb::value::Value::DatetimeValue(pb::DateTime {
                year: dt.year as u32,
                month: dt.month as u32,
                day: dt.day as u32,
                hour: dt.hour as u32,
                minute: dt.minute as u32,
                second: dt.second as u32,
                microsecond: dt.microsecond,
            })),
            V::Geography(g) => Some(pb::value::Value::GeographyValue(pb::Geography {
                wkt: g.to_string(),
            })),
            V::List(items) => Some(pb::value::Value::ListValue(pb::List {
                values: items.values.iter().map(Self::value_to_proto).collect(),
            })),
            V::Map(map) => Some(pb::value::Value::MapValue(pb::Map {
                values: map
                    .data
                    .iter()
                    .map(|(k, v)| (k.clone(), Self::value_to_proto(v)))
                    .collect(),
            })),
            V::Vertex(_) | V::Edge(_) | V::Path(_) => {
                // Complex types stored as string representation
                Some(pb::value::Value::StringValue(value.to_string()))
            }
            // Additional types - convert to string representation
            V::Empty => Some(pb::value::Value::NullValue(0)),
            V::Time(t) => Some(pb::value::Value::StringValue(t.to_string())),
            V::Set(s) => Some(pb::value::Value::StringValue(s.to_string())),
            V::DataSet(ds) => Some(pb::value::Value::StringValue(ds.to_string())),
            V::Duration(d) => Some(pb::value::Value::StringValue(d.to_string())),
        };

        pb::Value { value: inner }
    }

    fn decode_vertex_proto(data: &[u8]) -> Result<VertexData> {
        let proto_vertex = pb::Vertex::decode(data)
            .map_err(|e| CodecError::IncorrectValue(format!("Proto decode failed: {}", e)))?;

        Ok(VertexData {
            vid: proto_vertex.vid,
            tags: proto_vertex
                .tags
                .into_iter()
                .map(Self::proto_to_tag)
                .collect(),
        })
    }

    fn proto_to_tag(proto_tag: pb::Tag) -> TagData {
        TagData {
            name: proto_tag.name,
            properties: proto_tag
                .properties
                .into_iter()
                .map(|(k, v)| (k, Self::proto_to_value(v)))
                .collect(),
        }
    }

    fn decode_edge_proto(data: &[u8]) -> Result<EdgeData> {
        let proto_edge = pb::Edge::decode(data)
            .map_err(|e| CodecError::IncorrectValue(format!("Proto decode failed: {}", e)))?;

        Ok(EdgeData {
            src_vid: proto_edge.src_vid,
            dst_vid: proto_edge.dst_vid,
            edge_type: proto_edge.edge_type,
            ranking: proto_edge.ranking,
            properties: proto_edge
                .properties
                .into_iter()
                .map(|(k, v)| (k, Self::proto_to_value(v)))
                .collect(),
        })
    }

    fn proto_to_value(proto_value: pb::Value) -> byoridb_common::Value {
        use byoridb_common::types::NullType;
        use byoridb_common::Value as V;

        match proto_value.value {
            Some(pb::value::Value::NullValue(_)) => V::Null(NullType::Null),
            Some(pb::value::Value::BoolValue(b)) => V::Bool(b),
            Some(pb::value::Value::IntValue(i)) => V::Int(i),
            Some(pb::value::Value::FloatValue(f)) => V::Float(f),
            Some(pb::value::Value::StringValue(s)) => V::String(s),
            Some(pb::value::Value::DateValue(d)) => {
                V::Date(byoridb_common::datatypes::date::Date {
                    year: d.year as u16,
                    month: d.month as u8,
                    day: d.day as u8,
                })
            }
            Some(pb::value::Value::DatetimeValue(dt)) => {
                V::DateTime(byoridb_common::datatypes::datetime::DateTime {
                    year: dt.year as u16,
                    month: dt.month as u8,
                    day: dt.day as u8,
                    hour: dt.hour as u8,
                    minute: dt.minute as u8,
                    second: dt.second as u8,
                    microsecond: dt.microsecond,
                })
            }
            Some(pb::value::Value::GeographyValue(g)) => {
                V::Geography(byoridb_common::datatypes::geography::Geography::new(g.wkt))
            }
            Some(pb::value::Value::ListValue(list)) => V::List(CommonList::with_values(
                list.values.into_iter().map(Self::proto_to_value).collect(),
            )),
            Some(pb::value::Value::MapValue(map)) => V::Map(CommonMap {
                data: map
                    .values
                    .into_iter()
                    .map(|(k, v)| (k, Self::proto_to_value(v)))
                    .collect(),
            }),
            Some(pb::value::Value::BytesValue(b)) => {
                // Try to convert bytes to string
                V::String(String::from_utf8_lossy(&b).to_string())
            }
            None => V::Null(NullType::Null),
        }
    }

    // === JSON fallback for backward compatibility ===

    fn decode_vertex_json(data: &[u8]) -> Result<VertexData> {
        let json: serde_json::Value = serde_json::from_slice(data)
            .map_err(|e| CodecError::IncorrectValue(format!("JSON decode failed: {}", e)))?;

        let vid = json.get("vid").and_then(|v| v.as_i64()).unwrap_or_default();

        let tags = json
            .get("tags")
            .and_then(|t| t.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|tag| {
                        let name = tag.get("name")?.as_str()?.to_string();
                        let properties = tag
                            .get("props")
                            .and_then(|p| p.as_object())
                            .map(|obj| {
                                obj.iter()
                                    .map(|(k, v)| (k.clone(), Self::json_to_value(v)))
                                    .collect()
                            })
                            .unwrap_or_default();
                        Some(TagData { name, properties })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(VertexData { vid, tags })
    }

    fn decode_edge_json(data: &[u8]) -> Result<EdgeData> {
        let json: serde_json::Value = serde_json::from_slice(data)
            .map_err(|e| CodecError::IncorrectValue(format!("JSON decode failed: {}", e)))?;

        let src_vid = json.get("src").and_then(|v| v.as_i64()).unwrap_or_default();
        let dst_vid = json.get("dst").and_then(|v| v.as_i64()).unwrap_or_default();
        let edge_type = json
            .get("edge_type")
            .or_else(|| json.get("type"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let ranking = json
            .get("ranking")
            .or_else(|| json.get("rank"))
            .and_then(|v| v.as_i64())
            .unwrap_or_default();

        let properties = json
            .get("props")
            .or_else(|| json.get("properties"))
            .and_then(|p| p.as_object())
            .map(|obj| {
                obj.iter()
                    .map(|(k, v)| (k.clone(), Self::json_to_value(v)))
                    .collect()
            })
            .unwrap_or_default();

        Ok(EdgeData {
            src_vid,
            dst_vid,
            edge_type,
            ranking,
            properties,
        })
    }

    fn json_to_value(json: &serde_json::Value) -> byoridb_common::Value {
        use byoridb_common::types::NullType;
        use byoridb_common::Value as V;

        match json {
            serde_json::Value::Null => V::Null(NullType::Null),
            serde_json::Value::Bool(b) => V::Bool(*b),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    V::Int(i)
                } else if let Some(f) = n.as_f64() {
                    V::Float(f)
                } else {
                    V::Null(NullType::Null)
                }
            }
            serde_json::Value::String(s) => V::String(s.clone()),
            serde_json::Value::Array(arr) => V::List(CommonList::with_values(
                arr.iter().map(Self::json_to_value).collect(),
            )),
            serde_json::Value::Object(obj) => V::Map(CommonMap {
                data: obj
                    .iter()
                    .map(|(k, v)| (k.clone(), Self::json_to_value(v)))
                    .collect(),
            }),
        }
    }

    /// Convert VertexData to JSON (for backward compatibility output)
    pub fn vertex_to_json(vertex: &VertexData) -> serde_json::Value {
        serde_json::json!({
            "vid": vertex.vid,
            "tags": vertex.tags.iter().map(|tag| {
                serde_json::json!({
                    "name": tag.name,
                    "props": tag.properties.iter().map(|(k, v)| {
                        (k.clone(), Self::value_to_json(v))
                    }).collect::<serde_json::Map<String, serde_json::Value>>()
                })
            }).collect::<Vec<_>>()
        })
    }

    /// Convert EdgeData to JSON (for backward compatibility output)
    pub fn edge_to_json(edge: &EdgeData) -> serde_json::Value {
        serde_json::json!({
            "src": edge.src_vid,
            "dst": edge.dst_vid,
            "edge_type": edge.edge_type,
            "ranking": edge.ranking,
            "props": edge.properties.iter().map(|(k, v)| {
                (k.clone(), Self::value_to_json(v))
            }).collect::<serde_json::Map<String, serde_json::Value>>()
        })
    }

    fn value_to_json(value: &byoridb_common::Value) -> serde_json::Value {
        use byoridb_common::Value as V;

        match value {
            V::Null(_) => serde_json::Value::Null,
            V::Bool(b) => serde_json::Value::Bool(*b),
            V::Int(i) => serde_json::json!(*i),
            V::Float(f) => serde_json::json!(*f),
            V::String(s) => serde_json::Value::String(s.clone()),
            V::Date(d) => {
                serde_json::Value::String(format!("{}-{:02}-{:02}", d.year, d.month, d.day))
            }
            V::DateTime(dt) => serde_json::Value::String(format!(
                "{}-{:02}-{:02}T{:02}:{:02}:{:02}.{:06}",
                dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second, dt.microsecond
            )),
            V::Geography(g) => serde_json::Value::String(g.to_string()),
            V::List(items) => {
                serde_json::Value::Array(items.values.iter().map(Self::value_to_json).collect())
            }
            V::Map(map) => serde_json::Value::Object(
                map.data
                    .iter()
                    .map(|(k, v)| (k.clone(), Self::value_to_json(v)))
                    .collect(),
            ),
            V::Vertex(_) | V::Edge(_) | V::Path(_) => serde_json::Value::String(value.to_string()),
            // Additional types - convert to string representation
            V::Empty => serde_json::Value::Null,
            V::Time(t) => serde_json::Value::String(t.to_string()),
            V::Set(s) => serde_json::Value::String(s.to_string()),
            V::DataSet(ds) => serde_json::Value::String(ds.to_string()),
            V::Duration(d) => serde_json::Value::String(d.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use byoridb_common::Value;

    #[test]
    fn test_vertex_encode_decode() {
        let vertex = VertexData {
            vid: 123,
            tags: vec![TagData {
                name: "person".to_string(),
                properties: [
                    ("name".to_string(), Value::String("Alice".to_string())),
                    ("age".to_string(), Value::Int(30)),
                ]
                .into_iter()
                .collect(),
            }],
        };

        // Encode
        let bytes = VertexCodec::encode_vertex(&vertex).unwrap();
        assert!(VertexCodec::is_proto_format(&bytes));

        // Decode
        let decoded = VertexCodec::decode_vertex(&bytes).unwrap();
        assert_eq!(decoded.vid, vertex.vid);
        assert_eq!(decoded.tags.len(), 1);
        assert_eq!(decoded.tags[0].name, "person");
    }

    #[test]
    fn test_edge_encode_decode() {
        let edge = EdgeData {
            src_vid: 1,
            dst_vid: 2,
            edge_type: "follow".to_string(),
            ranking: 0,
            properties: [("since".to_string(), Value::Int(2020))]
                .into_iter()
                .collect(),
        };

        let bytes = VertexCodec::encode_edge(&edge).unwrap();
        assert!(VertexCodec::is_proto_format(&bytes));

        let decoded = VertexCodec::decode_edge(&bytes).unwrap();
        assert_eq!(decoded.src_vid, 1);
        assert_eq!(decoded.dst_vid, 2);
        assert_eq!(decoded.edge_type, "follow");
    }

    #[test]
    fn test_json_fallback_vertex() {
        // JSON format (legacy)
        let json = r#"{"vid": 456, "tags": [{"name": "player", "props": {"score": 100}}]}"#;
        let bytes = json.as_bytes();

        assert!(!VertexCodec::is_proto_format(bytes));

        let decoded = VertexCodec::decode_vertex(bytes).unwrap();
        assert_eq!(decoded.vid, 456);
        assert_eq!(decoded.tags[0].name, "player");
    }

    #[test]
    fn test_json_fallback_edge() {
        let json = r#"{"src": 10, "dst": 20, "props": {"weight": 1.5}}"#;
        let bytes = json.as_bytes();

        let decoded = VertexCodec::decode_edge(bytes).unwrap();
        assert_eq!(decoded.src_vid, 10);
        assert_eq!(decoded.dst_vid, 20);
    }

    #[test]
    fn test_decode_edge_dst_proto_matches_full_decode() {
        let edge = EdgeData {
            src_vid: 12345,
            dst_vid: 67890,
            edge_type: "follow".to_string(),
            ranking: 7,
            properties: [
                ("weight".to_string(), Value::Float(1.5)),
                ("note".to_string(), Value::String("hi".to_string())),
            ]
            .into_iter()
            .collect(),
        };
        let bytes = VertexCodec::encode_edge(&edge).unwrap();

        let full = VertexCodec::decode_edge(&bytes).unwrap();
        let fast = VertexCodec::decode_edge_dst(&bytes).unwrap();

        assert_eq!(full.dst_vid, 67890);
        assert_eq!(fast, full.dst_vid);
    }

    #[test]
    fn test_decode_edge_dst_proto_handles_zero_default() {
        // Edge with dst_vid omitted (proto default 0).
        let edge = EdgeData {
            src_vid: 1,
            dst_vid: 0,
            edge_type: "self".to_string(),
            ranking: 0,
            properties: Default::default(),
        };
        let bytes = VertexCodec::encode_edge(&edge).unwrap();

        let fast = VertexCodec::decode_edge_dst(&bytes).unwrap();
        assert_eq!(fast, 0);
    }

    #[test]
    fn test_decode_edge_dst_json_fallback() {
        let json = r#"{"src": 10, "dst": 20, "props": {"weight": 1.5}}"#;
        let dst = VertexCodec::decode_edge_dst(json.as_bytes()).unwrap();
        assert_eq!(dst, 20);
    }

    #[test]
    fn test_decode_edge_src_proto_matches_full_decode() {
        let edge = EdgeData {
            src_vid: 12345,
            dst_vid: 67890,
            edge_type: "follow".to_string(),
            ranking: 7,
            properties: [("weight".to_string(), Value::Float(1.5))]
                .into_iter()
                .collect(),
        };
        let bytes = VertexCodec::encode_edge(&edge).unwrap();

        let full = VertexCodec::decode_edge(&bytes).unwrap();
        let fast = VertexCodec::decode_edge_src(&bytes).unwrap();

        assert_eq!(full.src_vid, 12345);
        assert_eq!(fast, full.src_vid);
    }

    #[test]
    fn test_decode_edge_src_json_fallback() {
        let json = r#"{"src": 10, "dst": 20, "props": {"weight": 1.5}}"#;
        let src = VertexCodec::decode_edge_src(json.as_bytes()).unwrap();
        assert_eq!(src, 10);
    }

    #[test]
    fn test_size_comparison() {
        let vertex = VertexData {
            vid: 123456789,
            tags: vec![
                TagData {
                    name: "person".to_string(),
                    properties: [
                        ("name".to_string(), Value::String("Alice Smith".to_string())),
                        ("age".to_string(), Value::Int(30)),
                        (
                            "email".to_string(),
                            Value::String("alice@example.com".to_string()),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                },
                TagData {
                    name: "employee".to_string(),
                    properties: [
                        ("company".to_string(), Value::String("TechCorp".to_string())),
                        (
                            "department".to_string(),
                            Value::String("Engineering".to_string()),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                },
            ],
        };

        // Proto size
        let proto_bytes = VertexCodec::encode_vertex(&vertex).unwrap();

        // JSON size
        let json = VertexCodec::vertex_to_json(&vertex);
        let json_bytes = serde_json::to_vec(&json).unwrap();

        println!("Proto size: {} bytes", proto_bytes.len());
        println!("JSON size: {} bytes", json_bytes.len());
        println!(
            "Savings: {:.1}%",
            (1.0 - (proto_bytes.len() as f64 / json_bytes.len() as f64)) * 100.0
        );

        // Proto should be smaller
        assert!(proto_bytes.len() < json_bytes.len());
    }
}

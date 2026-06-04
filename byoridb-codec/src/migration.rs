// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Migration utilities for converting between data formats
//!
//! This module provides tools for migrating existing JSON-encoded data
//! to the more efficient Protocol Buffers format.
//!
//! # Usage
//! ```ignore
//! use byoridb_codec::migration::MigrationTool;
//!
//! // Check if data needs migration
//! let needs_migration = MigrationTool::needs_migration(&old_data);
//!
//! // Convert vertex data from JSON to Proto
//! let proto_data = MigrationTool::migrate_vertex(&json_data)?;
//! ```

use crate::error::{CodecError, Result};
use crate::vertex::{EdgeData, TagData, VertexCodec, VertexData};
use byoridb_common::datatypes::list::List as CommonList;
use byoridb_common::datatypes::map::Map as CommonMap;
use byoridb_common::types::NullType;
use byoridb_common::Value;

/// Migration tool for converting data between formats
pub struct MigrationTool;

/// Statistics from a migration run
#[derive(Debug, Default)]
pub struct MigrationStats {
    pub vertices_migrated: u64,
    pub edges_migrated: u64,
    pub vertices_skipped: u64,
    pub edges_skipped: u64,
    pub bytes_saved: i64,
    pub errors: Vec<String>,
}

impl MigrationTool {
    /// Check if data is in JSON format and needs migration to Proto
    pub fn needs_migration(data: &[u8]) -> bool {
        !VertexCodec::is_proto_format(data)
    }

    /// Migrate vertex data from JSON to Proto format
    /// Returns the new Proto-encoded data if migration was needed,
    /// or the original data if already in Proto format
    pub fn migrate_vertex(data: &[u8]) -> Result<Vec<u8>> {
        if VertexCodec::is_proto_format(data) {
            // Already Proto, no migration needed
            return Ok(data.to_vec());
        }

        // Decode from JSON
        let vertex = VertexCodec::decode_vertex(data)?;

        // Encode to Proto
        VertexCodec::encode_vertex(&vertex)
    }

    /// Migrate edge data from JSON to Proto format
    /// Returns the new Proto-encoded data if migration was needed,
    /// or the original data if already in Proto format
    pub fn migrate_edge(data: &[u8]) -> Result<Vec<u8>> {
        if VertexCodec::is_proto_format(data) {
            // Already Proto, no migration needed
            return Ok(data.to_vec());
        }

        // Decode from JSON
        let edge = VertexCodec::decode_edge(data)?;

        // Encode to Proto
        VertexCodec::encode_edge(&edge)
    }

    /// Estimate storage savings from migrating a single vertex
    /// Returns (json_size, proto_size, savings_bytes)
    pub fn estimate_vertex_savings(data: &[u8]) -> Result<(usize, usize, i64)> {
        if VertexCodec::is_proto_format(data) {
            // Already Proto, no savings
            return Ok((data.len(), data.len(), 0));
        }

        let proto_data = Self::migrate_vertex(data)?;
        let savings = data.len() as i64 - proto_data.len() as i64;
        Ok((data.len(), proto_data.len(), savings))
    }

    /// Estimate storage savings from migrating a single edge
    /// Returns (json_size, proto_size, savings_bytes)
    pub fn estimate_edge_savings(data: &[u8]) -> Result<(usize, usize, i64)> {
        if VertexCodec::is_proto_format(data) {
            // Already Proto, no savings
            return Ok((data.len(), data.len(), 0));
        }

        let proto_data = Self::migrate_edge(data)?;
        let savings = data.len() as i64 - proto_data.len() as i64;
        Ok((data.len(), proto_data.len(), savings))
    }

    /// Convert raw JSON bytes to VertexData
    pub fn json_to_vertex(json_bytes: &[u8]) -> Result<VertexData> {
        let json: serde_json::Value = serde_json::from_slice(json_bytes)
            .map_err(|e| CodecError::IncorrectValue(format!("JSON parse failed: {}", e)))?;

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

    /// Convert raw JSON bytes to EdgeData
    pub fn json_to_edge(json_bytes: &[u8]) -> Result<EdgeData> {
        let json: serde_json::Value = serde_json::from_slice(json_bytes)
            .map_err(|e| CodecError::IncorrectValue(format!("JSON parse failed: {}", e)))?;

        let src_vid = json.get("src").and_then(|v| v.as_i64()).unwrap_or_default();
        let dst_vid = json.get("dst").and_then(|v| v.as_i64()).unwrap_or_default();
        let edge_type = json
            .get("edge_type")
            .or_else(|| json.get("type"))
            .and_then(|v| v.as_str().or_else(|| v.as_i64().map(|_| "")))
            .unwrap_or_default()
            .to_string();
        // Handle edge_type as either string or number
        let edge_type = if edge_type.is_empty() {
            json.get("edge_type")
                .or_else(|| json.get("type"))
                .and_then(|v| v.as_i64())
                .map(|n| n.to_string())
                .unwrap_or_default()
        } else {
            edge_type
        };
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

    fn json_to_value(json: &serde_json::Value) -> Value {
        match json {
            serde_json::Value::Null => Value::Null(NullType::Null),
            serde_json::Value::Bool(b) => Value::Bool(*b),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Value::Int(i)
                } else if let Some(f) = n.as_f64() {
                    Value::Float(f)
                } else {
                    Value::Null(NullType::Null)
                }
            }
            serde_json::Value::String(s) => Value::String(s.clone()),
            serde_json::Value::Array(arr) => Value::List(CommonList::with_values(
                arr.iter().map(Self::json_to_value).collect(),
            )),
            serde_json::Value::Object(obj) => Value::Map(CommonMap {
                data: obj
                    .iter()
                    .map(|(k, v)| (k.clone(), Self::json_to_value(v)))
                    .collect(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_needs_migration_json() {
        let json = r#"{"vid": 123, "tags": []}"#;
        assert!(MigrationTool::needs_migration(json.as_bytes()));
    }

    #[test]
    fn test_needs_migration_proto() {
        // Create a Proto-encoded vertex
        let vertex = VertexData {
            vid: 123,
            tags: vec![],
        };
        let proto_data = VertexCodec::encode_vertex(&vertex).unwrap();
        assert!(!MigrationTool::needs_migration(&proto_data));
    }

    #[test]
    fn test_migrate_vertex() {
        let json = r#"{"vid": 456, "tags": [{"name": "person", "props": {"age": 30}}]}"#;
        let json_bytes = json.as_bytes();

        // Verify it needs migration
        assert!(MigrationTool::needs_migration(json_bytes));

        // Migrate
        let proto_data = MigrationTool::migrate_vertex(json_bytes).unwrap();

        // Verify result is Proto format
        assert!(!MigrationTool::needs_migration(&proto_data));

        // Verify data integrity
        let decoded = VertexCodec::decode_vertex(&proto_data).unwrap();
        assert_eq!(decoded.vid, 456);
        assert_eq!(decoded.tags.len(), 1);
        assert_eq!(decoded.tags[0].name, "person");
    }

    #[test]
    fn test_migrate_edge() {
        let json = r#"{"src": 1, "dst": 2, "edge_type": "follow", "ranking": 0, "props": {"weight": 1.5}}"#;
        let json_bytes = json.as_bytes();

        // Migrate
        let proto_data = MigrationTool::migrate_edge(json_bytes).unwrap();

        // Verify result is Proto format
        assert!(!MigrationTool::needs_migration(&proto_data));

        // Verify data integrity
        let decoded = VertexCodec::decode_edge(&proto_data).unwrap();
        assert_eq!(decoded.src_vid, 1);
        assert_eq!(decoded.dst_vid, 2);
    }

    #[test]
    fn test_json_to_edge_with_string_edge_type() {
        let json =
            r#"{"src": 10, "dst": 20, "edge_type": "follow", "rank": 5, "props": {"w": 0.5}}"#;
        let e = MigrationTool::json_to_edge(json.as_bytes()).unwrap();
        assert_eq!(e.src_vid, 10);
        assert_eq!(e.dst_vid, 20);
        assert_eq!(e.edge_type, "follow");
        assert_eq!(e.ranking, 5);
        assert!(e.properties.contains_key("w"));
    }

    #[test]
    fn test_json_to_edge_with_numeric_edge_type() {
        let json = r#"{"src": 1, "dst": 2, "type": 7, "ranking": 0}"#;
        let e = MigrationTool::json_to_edge(json.as_bytes()).unwrap();
        assert_eq!(e.edge_type, "7");
        assert_eq!(e.ranking, 0);
    }

    #[test]
    fn test_json_to_edge_missing_fields_use_defaults() {
        let json = r#"{}"#;
        let e = MigrationTool::json_to_edge(json.as_bytes()).unwrap();
        assert_eq!(e.src_vid, 0);
        assert_eq!(e.dst_vid, 0);
        assert_eq!(e.ranking, 0);
        assert!(e.properties.is_empty());
    }

    #[test]
    fn test_json_to_edge_invalid_json_returns_err() {
        let bad = b"not-json";
        assert!(MigrationTool::json_to_edge(bad).is_err());
    }

    #[test]
    fn test_json_to_value_covers_all_variants() {
        // Round-trip via vertex props since json_to_value is private
        let json = r#"{
            "vid": 1,
            "tags": [{
                "name": "t",
                "props": {
                    "n": null,
                    "b": true,
                    "i": 42,
                    "f": 3.14,
                    "s": "hi",
                    "arr": [1, 2, 3],
                    "obj": {"k": "v"}
                }
            }]
        }"#;
        let v = MigrationTool::json_to_vertex(json.as_bytes()).unwrap();
        let props = &v.tags[0].properties;
        assert!(matches!(props.get("n").unwrap(), Value::Null(_)));
        assert!(matches!(props.get("b").unwrap(), Value::Bool(true)));
        assert!(matches!(props.get("i").unwrap(), Value::Int(42)));
        assert!(matches!(props.get("f").unwrap(), Value::Float(_)));
        assert!(matches!(props.get("s").unwrap(), Value::String(s) if s == "hi"));
        assert!(matches!(props.get("arr").unwrap(), Value::List(_)));
        assert!(matches!(props.get("obj").unwrap(), Value::Map(_)));
    }

    #[test]
    fn test_estimate_savings() {
        let json = r#"{"vid": 789, "tags": [{"name": "test", "props": {"field1": "value1", "field2": 12345}}]}"#;
        let json_bytes = json.as_bytes();

        let (json_size, proto_size, savings) =
            MigrationTool::estimate_vertex_savings(json_bytes).unwrap();

        println!("JSON size: {} bytes", json_size);
        println!("Proto size: {} bytes", proto_size);
        println!(
            "Savings: {} bytes ({:.1}%)",
            savings,
            (savings as f64 / json_size as f64) * 100.0
        );

        // Proto should be smaller
        assert!(proto_size < json_size);
        assert!(savings > 0);
    }
}

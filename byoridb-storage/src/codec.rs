use crate::error::Result;
use byoridb_codec::{EdgeData, TagData, VertexCodec, VertexData};
use byoridb_common::Value;
use std::collections::HashMap;

pub struct Codec;

impl Codec {
    /// Encode a Value (properties) using bincode
    pub fn encode(value: &Value) -> Result<Vec<u8>> {
        // Use bincode for simple serialization of Rust types
        // Row format encoding implementation
        bincode::serialize(value)
            .map_err(|e| crate::error::StorageError::EncodingError(e.to_string()))
    }

    /// Decode a Value (properties) using bincode
    pub fn decode(bytes: &[u8]) -> Result<Value> {
        bincode::deserialize(bytes)
            .map_err(|e| crate::error::StorageError::DecodingError(e.to_string()))
    }

    /// Encode vertex data using Protocol Buffers
    pub fn encode_vertex(vid: i64, tags: Vec<(String, HashMap<String, Value>)>) -> Result<Vec<u8>> {
        let vertex_data = VertexData {
            vid,
            tags: tags
                .into_iter()
                .map(|(name, properties)| TagData { name, properties })
                .collect(),
        };
        VertexCodec::encode_vertex(&vertex_data)
            .map_err(|e| crate::error::StorageError::EncodingError(e.to_string()))
    }

    /// Decode vertex data (auto-detects Proto or JSON format)
    pub fn decode_vertex(bytes: &[u8]) -> Result<VertexData> {
        VertexCodec::decode_vertex(bytes)
            .map_err(|e| crate::error::StorageError::DecodingError(e.to_string()))
    }

    /// Encode edge data using Protocol Buffers
    pub fn encode_edge(
        src_vid: i64,
        dst_vid: i64,
        edge_type: String,
        ranking: i64,
        properties: HashMap<String, Value>,
    ) -> Result<Vec<u8>> {
        let edge_data = EdgeData {
            src_vid,
            dst_vid,
            edge_type,
            ranking,
            properties,
        };
        VertexCodec::encode_edge(&edge_data)
            .map_err(|e| crate::error::StorageError::EncodingError(e.to_string()))
    }

    /// Decode edge data (auto-detects Proto or JSON format)
    pub fn decode_edge(bytes: &[u8]) -> Result<EdgeData> {
        VertexCodec::decode_edge(bytes)
            .map_err(|e| crate::error::StorageError::DecodingError(e.to_string()))
    }

    /// Check if data is in Proto format
    pub fn is_proto_format(bytes: &[u8]) -> bool {
        VertexCodec::is_proto_format(bytes)
    }
}

// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Codec module for encoding and decoding graph data
//!
//! This module provides serialization/deserialization for:
//! - Row data with schema support
//! - Vertex and Edge data (Protocol Buffers with JSON fallback)
//! - Property values

pub mod error;
pub mod migration;
pub mod row;
pub mod schema;
pub mod vertex;

/// Generated protobuf types for data serialization
pub mod proto {
    pub mod data {
        include!(concat!(env!("OUT_DIR"), "/data.rs"));
    }
}

pub use error::{CodecError, Result};
pub use migration::{MigrationStats, MigrationTool};
pub use row::{RowReader, RowWriter};
pub use schema::{MemorySchemaProvider, PropertyType, Schema, SchemaProvider};
pub use vertex::{EdgeData, TagData, VertexCodec, VertexData};

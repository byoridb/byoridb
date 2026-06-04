// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Schema definitions for row encoding/decoding

use byoridb_common::{types::ValueType, Value};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PropertyType {
    Bool,
    Int8,
    Int16,
    Int32,
    Int64,
    Float,
    Double,
    String,
    FixedString(usize),
    Timestamp,
    Date,
    DateTime,
    Geography,
}

impl PropertyType {
    pub fn size(&self) -> usize {
        match self {
            PropertyType::Bool => 1,
            PropertyType::Int8 => 1,
            PropertyType::Int16 => 2,
            PropertyType::Int32 => 4,
            PropertyType::Int64 => 8,
            PropertyType::Float => 4,
            PropertyType::Double => 8,
            PropertyType::String => 8, // offset (4) + length (4)
            PropertyType::FixedString(len) => *len,
            PropertyType::Timestamp => 8,
            PropertyType::Date => 4,
            PropertyType::DateTime => 15,
            PropertyType::Geography => 8,
        }
    }

    pub fn is_variable_length(&self) -> bool {
        matches!(self, PropertyType::String | PropertyType::Geography)
    }
}

impl From<ValueType> for PropertyType {
    fn from(value_type: ValueType) -> Self {
        match value_type {
            ValueType::Bool => PropertyType::Bool,
            ValueType::Int => PropertyType::Int64,
            ValueType::Float => PropertyType::Double,
            ValueType::String => PropertyType::String,
            ValueType::Date => PropertyType::Date,
            ValueType::DateTime => PropertyType::DateTime,
            ValueType::Geography => PropertyType::Geography,
            _ => PropertyType::String, // Default fallback
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PropertyDef {
    pub name: String,
    pub prop_type: PropertyType,
    pub nullable: bool,
    pub default_value: Option<Value>,
}

impl PropertyDef {
    pub fn new(name: impl Into<String>, prop_type: PropertyType, nullable: bool) -> Self {
        PropertyDef {
            name: name.into(),
            prop_type,
            nullable,
            default_value: None,
        }
    }

    pub fn with_default(
        name: impl Into<String>,
        prop_type: PropertyType,
        nullable: bool,
        default_value: Option<Value>,
    ) -> Self {
        PropertyDef {
            name: name.into(),
            prop_type,
            nullable,
            default_value,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schema {
    pub version: i32,
    pub properties: Vec<PropertyDef>,
    pub prop_index: HashMap<String, usize>,
}

impl Schema {
    pub fn new(version: i32) -> Self {
        Schema {
            version,
            properties: Vec::new(),
            prop_index: HashMap::new(),
        }
    }

    pub fn add_property(&mut self, prop: PropertyDef) {
        let idx = self.properties.len();
        self.prop_index.insert(prop.name.clone(), idx);
        self.properties.push(prop);
    }

    pub fn get_property_index(&self, name: &str) -> Option<usize> {
        self.prop_index.get(name).copied()
    }

    pub fn get_property(&self, index: usize) -> Option<&PropertyDef> {
        self.properties.get(index)
    }

    pub fn num_nullables(&self) -> usize {
        self.properties.iter().filter(|p| p.nullable).count()
    }

    pub fn null_flags_size(&self) -> usize {
        if self.num_nullables() == 0 {
            return 0;
        }
        ((self.num_nullables() - 1) / 8) + 1
    }

    pub fn fixed_data_size(&self) -> usize {
        self.properties.iter().map(|p| p.prop_type.size()).sum()
    }
}

/// Schema provider trait
pub trait SchemaProvider: Send + Sync {
    fn get_schema(&self, version: i32) -> Option<Schema>;
    fn get_latest_version(&self) -> Option<i32>;
    fn get_current_schema(&self) -> Option<Schema> {
        self.get_latest_version().and_then(|v| self.get_schema(v))
    }
}

/// In-memory schema provider
pub struct MemorySchemaProvider {
    schemas: HashMap<i32, Schema>,
    latest_version: Option<i32>,
}

impl MemorySchemaProvider {
    pub fn new() -> Self {
        MemorySchemaProvider {
            schemas: HashMap::new(),
            latest_version: None,
        }
    }

    pub fn add_schema(&mut self, schema: Schema) {
        if self.latest_version.is_none() || self.latest_version < Some(schema.version) {
            self.latest_version = Some(schema.version);
        }
        self.schemas.insert(schema.version, schema);
    }
}

impl Default for MemorySchemaProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl SchemaProvider for MemorySchemaProvider {
    fn get_schema(&self, version: i32) -> Option<Schema> {
        self.schemas.get(&version).cloned()
    }

    fn get_latest_version(&self) -> Option<i32> {
        self.latest_version
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_property_def_defaults() {
        // Test without default
        let p1 = PropertyDef::new("p1", PropertyType::Int64, true);
        assert!(p1.default_value.is_none());
        assert!(p1.nullable);

        // Test with default
        let default_val = Some(Value::Int(42));
        let p2 = PropertyDef::with_default("p2", PropertyType::Int64, false, default_val.clone());
        assert_eq!(p2.default_value, default_val);
        assert!(!p2.nullable);

        // Test with default None
        let p3 = PropertyDef::with_default("p3", PropertyType::String, true, None);
        assert!(p3.default_value.is_none());
        assert!(p3.nullable);
    }

    #[test]
    fn test_property_type_size_fixed_widths() {
        assert_eq!(PropertyType::Bool.size(), 1);
        assert_eq!(PropertyType::Int8.size(), 1);
        assert_eq!(PropertyType::Int16.size(), 2);
        assert_eq!(PropertyType::Int32.size(), 4);
        assert_eq!(PropertyType::Int64.size(), 8);
        assert_eq!(PropertyType::Float.size(), 4);
        assert_eq!(PropertyType::Double.size(), 8);
        assert_eq!(PropertyType::Timestamp.size(), 8);
        assert_eq!(PropertyType::Date.size(), 4);
        assert_eq!(PropertyType::DateTime.size(), 15);
    }

    #[test]
    fn test_property_type_size_variable_uses_offset_plus_length() {
        // String/Geography use 8 bytes (4 offset + 4 length) in fixed area
        assert_eq!(PropertyType::String.size(), 8);
        assert_eq!(PropertyType::Geography.size(), 8);
    }

    #[test]
    fn test_property_type_size_fixed_string_uses_arg() {
        assert_eq!(PropertyType::FixedString(16).size(), 16);
        assert_eq!(PropertyType::FixedString(0).size(), 0);
    }

    #[test]
    fn test_is_variable_length() {
        assert!(PropertyType::String.is_variable_length());
        assert!(PropertyType::Geography.is_variable_length());
        // Everything else is fixed
        for pt in [
            PropertyType::Bool,
            PropertyType::Int64,
            PropertyType::Double,
            PropertyType::FixedString(10),
            PropertyType::Date,
            PropertyType::DateTime,
            PropertyType::Timestamp,
        ] {
            assert!(!pt.is_variable_length(), "{:?} should be fixed", pt);
        }
    }

    #[test]
    fn test_from_value_type_for_property_type() {
        assert_eq!(PropertyType::from(ValueType::Bool), PropertyType::Bool);
        assert_eq!(PropertyType::from(ValueType::Int), PropertyType::Int64);
        assert_eq!(PropertyType::from(ValueType::Float), PropertyType::Double);
        assert_eq!(PropertyType::from(ValueType::String), PropertyType::String);
        assert_eq!(PropertyType::from(ValueType::Date), PropertyType::Date);
        assert_eq!(
            PropertyType::from(ValueType::DateTime),
            PropertyType::DateTime
        );
        assert_eq!(
            PropertyType::from(ValueType::Geography),
            PropertyType::Geography
        );
        // Unsupported types fall back to String
        assert_eq!(PropertyType::from(ValueType::Vertex), PropertyType::String);
        assert_eq!(PropertyType::from(ValueType::Map), PropertyType::String);
    }

    #[test]
    fn test_null_flags_size_zero_when_no_nullables() {
        let mut s = Schema::new(1);
        s.add_property(PropertyDef::new("a", PropertyType::Int64, false));
        s.add_property(PropertyDef::new("b", PropertyType::Bool, false));
        assert_eq!(s.num_nullables(), 0);
        assert_eq!(s.null_flags_size(), 0);
    }

    #[test]
    fn test_null_flags_size_packs_8_per_byte() {
        // 1..=8 nullables → 1 byte; 9..=16 → 2 bytes; etc.
        for (n, expected_bytes) in [(1, 1), (7, 1), (8, 1), (9, 2), (16, 2), (17, 3)] {
            let mut s = Schema::new(1);
            for i in 0..n {
                s.add_property(PropertyDef::new(format!("p{i}"), PropertyType::Bool, true));
            }
            assert_eq!(
                s.null_flags_size(),
                expected_bytes,
                "n={n} should need {expected_bytes} bytes"
            );
        }
    }

    #[test]
    fn test_memory_schema_provider_add_tracks_latest() {
        let mut p = MemorySchemaProvider::new();
        assert!(p.get_latest_version().is_none());

        p.add_schema(Schema::new(1));
        assert_eq!(p.get_latest_version(), Some(1));

        // Higher version updates latest
        p.add_schema(Schema::new(3));
        assert_eq!(p.get_latest_version(), Some(3));

        // Adding lower version does NOT downgrade latest
        p.add_schema(Schema::new(2));
        assert_eq!(p.get_latest_version(), Some(3));

        // All schemas are retrievable
        assert!(p.get_schema(1).is_some());
        assert!(p.get_schema(2).is_some());
        assert!(p.get_schema(3).is_some());
        assert!(p.get_schema(99).is_none());
    }

    #[test]
    fn test_memory_schema_provider_get_current_schema() {
        let mut p = MemorySchemaProvider::new();
        p.add_schema(Schema::new(5));
        let cur = p.get_current_schema().unwrap();
        assert_eq!(cur.version, 5);
    }
}

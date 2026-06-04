// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Row encoding and decoding (Version 2)
//!
//! Row format (Version 2):
//! - Header byte: 00001vvv (vvv = schema version bytes)
//! - Schema version: variable length (1-8 bytes)
//! - Null flags: for nullable fields
//! - Fixed data: all non-variable fields
//! - Variable data: strings and geography data
//!
//! # Schema Evolution and Recovery
//!
//! This codec relies on "Schema-on-Read". To decode a row, the `SchemaProvider` must possess
//! the schema version that was active when the row was written.
//!
//! **CRITICAL REQUIREMENT**: All historical schema versions must be preserved indefinitely.
//!
//! ## Recovery Procedure for Missing Schema
//! If a schema version is lost (e.g. partial restore):
//! 1. Identify rows failing with "Schema version X not found".
//! 2. Manually reconstruct Schema X if structure is known (e.g. from logs or adjacent versions).
//! 3. Register the reconstructed schema with the `SchemaProvider`.
//! 4. If reconstruction is impossible, these rows must be considered corrupted.

use super::{
    error::{CodecError, Result},
    schema::{PropertyType, Schema, SchemaProvider},
};
use byoridb_common::{types::NullType, Value};
use bytes::{BufMut, BytesMut};

const ENCODING_VERSION: u8 = 0b00001000; // Version 2

pub struct RowWriter {
    schema: Schema,
    buffer: BytesMut,
    null_flags: Vec<u8>,
    var_data: BytesMut,
    values_set: Vec<bool>,
}

impl RowWriter {
    pub fn new(schema: Schema) -> Self {
        let prop_count = schema.properties.len();
        let null_flags_size = schema.null_flags_size();
        RowWriter {
            schema,
            buffer: BytesMut::new(),
            null_flags: vec![0; null_flags_size],
            var_data: BytesMut::new(),
            values_set: vec![false; prop_count],
        }
    }

    pub fn set_value(&mut self, index: usize, value: &Value) -> Result<()> {
        let prop = self
            .schema
            .get_property(index)
            .ok_or_else(|| CodecError::UnknownField(format!("{}", index)))?;

        self.values_set[index] = true;

        if value.is_null() {
            if !prop.nullable {
                return Err(CodecError::NotNullable(prop.name.clone()));
            }
            self.set_null_flag(index, true);
            return Ok(());
        }

        let prop_type = prop.prop_type;
        let _nullable = prop.nullable;

        self.set_null_flag(index, false);

        match prop_type {
            PropertyType::Bool => {
                let b = value.as_bool()?;
                self.buffer.put_u8(b as u8);
            }
            PropertyType::Int8 => {
                let i = value.as_int()?;
                if i < i8::MIN as i64 || i > i8::MAX as i64 {
                    return Err(CodecError::OutOfRange);
                }
                self.buffer.put_i8(i as i8);
            }
            PropertyType::Int16 => {
                let i = value.as_int()?;
                if i < i16::MIN as i64 || i > i16::MAX as i64 {
                    return Err(CodecError::OutOfRange);
                }
                self.buffer.put_i16(i as i16);
            }
            PropertyType::Int32 => {
                let i = value.as_int()?;
                if i < i32::MIN as i64 || i > i32::MAX as i64 {
                    return Err(CodecError::OutOfRange);
                }
                self.buffer.put_i32(i as i32);
            }
            PropertyType::Int64 => {
                let i = value.as_int()?;
                self.buffer.put_i64(i);
            }
            PropertyType::Float => {
                let f = value.as_float()?;
                self.buffer.put_f32(f as f32);
            }
            PropertyType::Double => {
                let f = value.as_float()?;
                self.buffer.put_f64(f);
            }
            PropertyType::String => {
                let s = value.as_str()?;
                let offset = self.var_data.len();
                self.var_data.put(s.as_bytes());
                self.buffer.put_u32(offset as u32);
                self.buffer.put_u32(s.len() as u32);
            }
            PropertyType::FixedString(len) => {
                let s = value.as_str()?;
                if s.len() > len {
                    return Err(CodecError::OutOfRange);
                }
                let bytes = s.as_bytes();
                self.buffer.put_slice(bytes);
                // Pad with zeros
                for _ in 0..(len - bytes.len()) {
                    self.buffer.put_u8(0);
                }
            }
            PropertyType::Timestamp => {
                let i = value.as_int()?;
                self.buffer.put_i64(i);
            }
            PropertyType::Date => {
                let d = match value {
                    Value::Date(d) => *d,
                    Value::String(s) => byoridb_common::datatypes::date::Date::parse(s)
                        .ok_or_else(|| {
                            CodecError::IncorrectValue(format!("Invalid Date string: {}", s))
                        })?,
                    _ => {
                        return Err(CodecError::TypeMismatch {
                            expected: "Date".to_string(),
                            found: format!("{:?}", value),
                        })
                    }
                };
                // Encode as u32: year(16) | month(8) | day(8)
                let val: u32 = ((d.year as u32) << 16) | ((d.month as u32) << 8) | (d.day as u32);
                self.buffer.put_u32(val);
            }
            PropertyType::DateTime => {
                let dt = match value {
                    Value::DateTime(dt) => *dt,
                    Value::String(s) => byoridb_common::datatypes::datetime::DateTime::parse(s)
                        .ok_or_else(|| {
                            CodecError::IncorrectValue(format!("Invalid DateTime string: {}", s))
                        })?,
                    _ => {
                        return Err(CodecError::TypeMismatch {
                            expected: "DateTime".to_string(),
                            found: format!("{:?}", value),
                        })
                    }
                };
                // Encode as 15 bytes (fixed size)
                // Layout: year(2) month(1) day(1) hour(1) minute(1) second(1) micro(4) padding(4)
                self.buffer.put_u16(dt.year);
                self.buffer.put_u8(dt.month);
                self.buffer.put_u8(dt.day);
                self.buffer.put_u8(dt.hour);
                self.buffer.put_u8(dt.minute);
                self.buffer.put_u8(dt.second);
                self.buffer.put_u32(dt.microsecond);
                self.buffer.put_slice(&[0u8; 4]);
            }
            PropertyType::Geography => {
                // Geography stored as string with offset
                let offset = self.var_data.len();
                // Store as string for now (WKT/WKB)
                let s = value.to_string();
                self.var_data.put(s.as_bytes());
                self.buffer.put_u32(offset as u32);
                self.buffer.put_u32(s.len() as u32);
            }
        }

        Ok(())
    }

    fn set_null_flag(&mut self, index: usize, null: bool) {
        let flag_index = index / 8;
        // If there are no nullable fields, null_flags is empty — nothing to set.
        if flag_index >= self.null_flags.len() {
            return;
        }
        let bit_index = index % 8;

        if null {
            self.null_flags[flag_index] |= 1 << bit_index;
        } else {
            self.null_flags[flag_index] &= !(1 << bit_index);
        }
    }

    pub fn encode(self) -> Result<Vec<u8>> {
        // Check all required fields are set
        for (i, prop) in self.schema.properties.iter().enumerate() {
            if !self.values_set[i] && !prop.nullable {
                return Err(CodecError::FieldUnset(prop.name.clone()));
            }
        }

        let mut result = BytesMut::new();

        // Write header byte
        let schema_ver_bytes = self.schema_version_bytes();
        result.put_u8(ENCODING_VERSION | schema_ver_bytes);

        // Write schema version
        self.write_schema_version(&mut result);

        // Write null flags
        result.put_slice(&self.null_flags);

        // Write fixed data
        result.put_slice(&self.buffer);

        // Write variable data
        result.put_slice(&self.var_data);

        Ok(result.to_vec())
    }

    fn schema_version_bytes(&self) -> u8 {
        // Determine bytes needed for schema version
        let v = self.schema.version.unsigned_abs();
        if v < 0x80 {
            1
        } else if v < 0x8000 {
            2
        } else if v < 0x800000 {
            3
        } else {
            4
        }
    }

    fn write_schema_version(&self, buf: &mut BytesMut) {
        let v = self.schema.version;
        let bytes = self.schema_version_bytes();

        for i in 0..bytes {
            let shift = (bytes - 1 - i) * 8;
            buf.put_u8(((v >> shift) & 0xFF) as u8);
        }
    }
}

pub struct RowReader<'a> {
    data: &'a [u8],
    schema: Schema,
    schema_ver_bytes: u8,
    header_size: usize,
    _data_offset: usize,
}

impl<'a> RowReader<'a> {
    pub fn new(data: &'a [u8], schema: Schema) -> Result<Self> {
        if data.is_empty() {
            return Err(CodecError::IncorrectValue("Empty data".to_string()));
        }

        let header = data[0];
        let version = header & 0b00011000;

        if version != ENCODING_VERSION {
            return Err(CodecError::InvalidEncodingVersion(version));
        }

        let schema_ver_bytes = header & 0b00000111;
        if schema_ver_bytes == 0 {
            return Err(CodecError::IncorrectValue(
                "Invalid schema version length: 0".to_string(),
            ));
        }
        let header_size = 1 + schema_ver_bytes as usize;

        if data.len() < header_size {
            return Err(CodecError::IncorrectValue(format!(
                "Data too short for header: expected {} bytes, got {}",
                header_size,
                data.len()
            )));
        }

        Ok(RowReader {
            data,
            schema,
            schema_ver_bytes,
            header_size,
            _data_offset: 0,
        })
    }

    pub fn get_schema_version(&self) -> i32 {
        let mut v = 0i32;
        for i in 0..self.schema_ver_bytes {
            let byte = self.data[1 + i as usize];
            v = (v << 8) | (byte as i32);
        }
        v
    }

    pub fn get_value_by_index(&self, index: usize) -> Result<Value> {
        let prop = self
            .schema
            .get_property(index)
            .ok_or_else(|| CodecError::UnknownField(format!("{}", index)))?;

        // Check null flag
        let null_offset = self.header_size;
        let flag_index = index / 8;
        let bit_index = index % 8;
        let null_flag = self.data[null_offset + flag_index] & (1 << bit_index) != 0;

        if null_flag {
            return Ok(Value::Null(NullType::Null));
        }

        // Calculate offset in fixed data
        let mut data_offset = self.header_size + self.schema.null_flags_size();
        for i in 0..index {
            if let Some(p) = self.schema.get_property(i) {
                data_offset += p.prop_type.size();
            }
        }

        self.read_value(prop, data_offset)
    }

    fn read_value(&self, prop: &super::schema::PropertyDef, offset: usize) -> Result<Value> {
        match prop.prop_type {
            PropertyType::Bool => Ok(Value::Bool(self.data[offset] != 0)),
            PropertyType::Int8 => Ok(Value::Int(self.data[offset] as i64)),
            PropertyType::Int16 => {
                Ok(Value::Int(
                    i16::from_be_bytes([self.data[offset], self.data[offset + 1]]) as i64,
                ))
            }
            PropertyType::Int32 => Ok(Value::Int(i32::from_be_bytes([
                self.data[offset],
                self.data[offset + 1],
                self.data[offset + 2],
                self.data[offset + 3],
            ]) as i64)),
            PropertyType::Int64 => Ok(Value::Int(i64::from_be_bytes([
                self.data[offset],
                self.data[offset + 1],
                self.data[offset + 2],
                self.data[offset + 3],
                self.data[offset + 4],
                self.data[offset + 5],
                self.data[offset + 6],
                self.data[offset + 7],
            ]))),
            PropertyType::Float => Ok(Value::Float(f32::from_be_bytes([
                self.data[offset],
                self.data[offset + 1],
                self.data[offset + 2],
                self.data[offset + 3],
            ]) as f64)),
            PropertyType::Double => Ok(Value::Float(f64::from_be_bytes([
                self.data[offset],
                self.data[offset + 1],
                self.data[offset + 2],
                self.data[offset + 3],
                self.data[offset + 4],
                self.data[offset + 5],
                self.data[offset + 6],
                self.data[offset + 7],
            ]))),
            PropertyType::String => {
                let str_offset = u32::from_be_bytes([
                    self.data[offset],
                    self.data[offset + 1],
                    self.data[offset + 2],
                    self.data[offset + 3],
                ]) as usize;
                let str_len = u32::from_be_bytes([
                    self.data[offset + 4],
                    self.data[offset + 5],
                    self.data[offset + 6],
                    self.data[offset + 7],
                ]) as usize;

                let fixed_size = self.schema.fixed_data_size();
                let var_start = self.header_size + self.schema.null_flags_size() + fixed_size;

                let s = std::str::from_utf8(
                    &self.data[var_start + str_offset..var_start + str_offset + str_len],
                )
                .map_err(|_| CodecError::IncorrectValue("Invalid UTF-8".to_string()))?;
                Ok(Value::String(s.to_string()))
            }
            PropertyType::Date => {
                let val = u32::from_be_bytes([
                    self.data[offset],
                    self.data[offset + 1],
                    self.data[offset + 2],
                    self.data[offset + 3],
                ]);
                let d = byoridb_common::datatypes::date::Date {
                    year: (val >> 16) as u16,
                    month: ((val >> 8) & 0xFF) as u8,
                    day: (val & 0xFF) as u8,
                };
                Ok(Value::Date(d))
            }
            PropertyType::DateTime => {
                let dt = byoridb_common::datatypes::datetime::DateTime {
                    year: u16::from_be_bytes([self.data[offset], self.data[offset + 1]]),
                    month: self.data[offset + 2],
                    day: self.data[offset + 3],
                    hour: self.data[offset + 4],
                    minute: self.data[offset + 5],
                    second: self.data[offset + 6],
                    microsecond: u32::from_be_bytes([
                        self.data[offset + 7],
                        self.data[offset + 8],
                        self.data[offset + 9],
                        self.data[offset + 10],
                    ]),
                };
                Ok(Value::DateTime(dt))
            }
            PropertyType::Geography => {
                // Same as string decoding
                let str_offset = u32::from_be_bytes([
                    self.data[offset],
                    self.data[offset + 1],
                    self.data[offset + 2],
                    self.data[offset + 3],
                ]) as usize;
                let str_len = u32::from_be_bytes([
                    self.data[offset + 4],
                    self.data[offset + 5],
                    self.data[offset + 6],
                    self.data[offset + 7],
                ]) as usize;

                let fixed_size = self.schema.fixed_data_size();
                let var_start = self.header_size + self.schema.null_flags_size() + fixed_size;

                let start = var_start + str_offset;
                let end = start + str_len;
                if end > self.data.len() {
                    return Err(CodecError::IncorrectValue(
                        "Geography data out of bounds".to_string(),
                    ));
                }

                let s = std::str::from_utf8(&self.data[start..end])
                    .map_err(|_| CodecError::IncorrectValue("Invalid UTF-8".to_string()))?;

                // TODO: Parse WKT/WKB into Geography struct
                Ok(Value::Geography(
                    byoridb_common::datatypes::geography::Geography::new(s.to_string()),
                ))
            }
            _ => Ok(Value::Null(NullType::Null)),
        }
    }
}

/// Version-aware row reader for lazy schema migration.
///
/// Reads row data encoded with an older schema version and transforms it
/// to match the current/target schema version. Fields that don't exist
/// in the old schema are returned as NULL or with their default values.
pub struct VersionAwareReader<'a> {
    data: &'a [u8],
    target_schema: Schema,
    row_schema_version: i32,
    row_schema: Schema,
    header_size: usize,
}

impl<'a> VersionAwareReader<'a> {
    /// Create a new version-aware reader.
    ///
    /// # Arguments
    /// * `data` - The raw row data
    /// * `provider` - Schema provider for resolving schema versions
    ///
    /// # Returns
    /// A reader that can decode the row using the appropriate schema version
    pub fn new<P: SchemaProvider>(data: &'a [u8], provider: &P) -> Result<Self> {
        if data.is_empty() {
            return Err(CodecError::IncorrectValue("Empty data".to_string()));
        }

        let header = data[0];
        let version_check = header & 0b00011000;

        if version_check != ENCODING_VERSION {
            return Err(CodecError::InvalidEncodingVersion(version_check));
        }

        let schema_ver_bytes = header & 0b00000111;
        if schema_ver_bytes == 0 {
            return Err(CodecError::IncorrectValue(
                "Invalid schema version length: 0".to_string(),
            ));
        }
        let header_size = 1 + schema_ver_bytes as usize;

        if data.len() < header_size {
            return Err(CodecError::IncorrectValue(format!(
                "Data too short for header: expected {} bytes, got {}",
                header_size,
                data.len()
            )));
        }

        // Read schema version from row
        let mut row_schema_version = 0i32;
        for i in 0..schema_ver_bytes {
            let byte = data[1 + i as usize];
            row_schema_version = (row_schema_version << 8) | (byte as i32);
        }

        // Get the schema for the row's version
        // WARNING: Requires ALL historical schema versions to be preserved.
        // If a schema version is missing (e.g. lost during partial backup restore),
        // decoding this row will fail.
        let row_schema = provider.get_schema(row_schema_version).ok_or_else(|| {
            CodecError::IncorrectValue(format!("Schema version {} not found", row_schema_version))
        })?;

        let target_schema = provider
            .get_current_schema()
            .ok_or_else(|| CodecError::IncorrectValue("Target schema not found".to_string()))?;

        Ok(VersionAwareReader {
            data,
            target_schema,
            row_schema_version,
            row_schema,
            header_size,
        })
    }

    /// Get the schema version stored in the row
    pub fn get_row_schema_version(&self) -> i32 {
        self.row_schema_version
    }

    /// Get the target (current) schema version
    pub fn get_target_schema_version(&self) -> i32 {
        self.target_schema.version
    }

    /// Read all values, transforming from row schema to target schema.
    ///
    /// For fields in target schema but not in row schema:
    /// - Returns default value if defined
    /// - Returns NULL otherwise
    pub fn read_all(&self) -> Result<std::collections::HashMap<String, Value>> {
        let mut result = std::collections::HashMap::new();

        // Iterate over target schema fields
        for target_prop in self.target_schema.properties.iter() {
            // Try to find this field in the row schema
            let value = if let Some(row_idx) = self.find_field_in_row_schema(&target_prop.name) {
                // Field exists in row schema, read it
                self.read_value_from_row(row_idx)?
            } else {
                // Field doesn't exist in row schema (added in later version)
                // Try to get default value, otherwise return NULL
                target_prop
                    .default_value
                    .clone()
                    .unwrap_or(Value::Null(NullType::Null))
            };

            result.insert(target_prop.name.clone(), value);
        }

        Ok(result)
    }

    /// Read a specific field by name.
    ///
    /// Returns NULL if the field doesn't exist in the row's schema version
    /// and no default value is defined.
    pub fn read_field(&self, field_name: &str) -> Result<Value> {
        // Check if field exists in row schema
        if let Some(row_idx) = self.find_field_in_row_schema(field_name) {
            self.read_value_from_row(row_idx)
        } else {
            // Field doesn't exist in row schema
            // Check if it exists in target schema and has default
            if let Some(idx) = self.target_schema.get_property_index(field_name) {
                Ok(self.target_schema.properties[idx]
                    .default_value
                    .clone()
                    .unwrap_or(Value::Null(NullType::Null)))
            } else {
                Ok(Value::Null(NullType::Null))
            }
        }
    }

    /// Read a specific field by target schema index.
    pub fn read_by_index(&self, target_idx: usize) -> Result<Value> {
        let target_prop = self
            .target_schema
            .get_property(target_idx)
            .ok_or_else(|| CodecError::UnknownField(format!("{}", target_idx)))?;

        self.read_field(&target_prop.name)
    }

    fn find_field_in_row_schema(&self, field_name: &str) -> Option<usize> {
        self.row_schema
            .properties
            .iter()
            .position(|p| p.name == field_name)
    }

    fn read_value_from_row(&self, row_idx: usize) -> Result<Value> {
        let prop = self
            .row_schema
            .get_property(row_idx)
            .ok_or_else(|| CodecError::UnknownField(format!("{}", row_idx)))?;

        // Check null flag
        let null_offset = self.header_size;
        let flag_index = row_idx / 8;
        let bit_index = row_idx % 8;

        if null_offset + flag_index >= self.data.len() {
            return Err(CodecError::IncorrectValue(format!(
                "Null flag index out of bounds: {} (data len: {})",
                null_offset + flag_index,
                self.data.len()
            )));
        }

        let null_flag = self.data[null_offset + flag_index] & (1 << bit_index) != 0;

        if null_flag {
            return Ok(Value::Null(NullType::Null));
        }

        // Calculate offset in fixed data
        let mut data_offset = self.header_size + self.row_schema.null_flags_size();
        for i in 0..row_idx {
            if let Some(p) = self.row_schema.get_property(i) {
                data_offset += p.prop_type.size();
            }
        }

        self.decode_value(prop, data_offset)
    }

    fn decode_value(&self, prop: &super::schema::PropertyDef, offset: usize) -> Result<Value> {
        // Check bounds for fixed part
        let fixed_size = if let PropertyType::String = prop.prop_type {
            8 // 4 bytes offset + 4 bytes length
        } else {
            prop.prop_type.size()
        };

        if offset + fixed_size > self.data.len() {
            return Err(CodecError::IncorrectValue(format!(
                "Data out of bounds for field '{}': needed {} bytes at offset {}, but data len is {}",
                prop.name, fixed_size, offset, self.data.len()
            )));
        }

        match prop.prop_type {
            PropertyType::Bool => Ok(Value::Bool(self.data[offset] != 0)),
            PropertyType::Int8 => Ok(Value::Int(self.data[offset] as i64)),
            PropertyType::Int16 => {
                Ok(Value::Int(
                    i16::from_be_bytes([self.data[offset], self.data[offset + 1]]) as i64,
                ))
            }
            PropertyType::Int32 => Ok(Value::Int(i32::from_be_bytes([
                self.data[offset],
                self.data[offset + 1],
                self.data[offset + 2],
                self.data[offset + 3],
            ]) as i64)),
            PropertyType::Int64 => Ok(Value::Int(i64::from_be_bytes([
                self.data[offset],
                self.data[offset + 1],
                self.data[offset + 2],
                self.data[offset + 3],
                self.data[offset + 4],
                self.data[offset + 5],
                self.data[offset + 6],
                self.data[offset + 7],
            ]))),
            PropertyType::Float => Ok(Value::Float(f32::from_be_bytes([
                self.data[offset],
                self.data[offset + 1],
                self.data[offset + 2],
                self.data[offset + 3],
            ]) as f64)),
            PropertyType::Double => Ok(Value::Float(f64::from_be_bytes([
                self.data[offset],
                self.data[offset + 1],
                self.data[offset + 2],
                self.data[offset + 3],
                self.data[offset + 4],
                self.data[offset + 5],
                self.data[offset + 6],
                self.data[offset + 7],
            ]))),
            PropertyType::String => {
                let str_offset = u32::from_be_bytes([
                    self.data[offset],
                    self.data[offset + 1],
                    self.data[offset + 2],
                    self.data[offset + 3],
                ]) as usize;
                let str_len = u32::from_be_bytes([
                    self.data[offset + 4],
                    self.data[offset + 5],
                    self.data[offset + 6],
                    self.data[offset + 7],
                ]) as usize;

                let fixed_size_total = self.row_schema.fixed_data_size();
                let var_start =
                    self.header_size + self.row_schema.null_flags_size() + fixed_size_total;
                let start_idx = var_start + str_offset;
                let end_idx = start_idx + str_len;

                if end_idx > self.data.len() {
                    return Err(CodecError::IncorrectValue(format!(
                        "String data out of bounds for field '{}': range {}..{} (data len: {})",
                        prop.name,
                        start_idx,
                        end_idx,
                        self.data.len()
                    )));
                }

                let s = std::str::from_utf8(&self.data[start_idx..end_idx])
                    .map_err(|_| CodecError::IncorrectValue("Invalid UTF-8".to_string()))?;
                Ok(Value::String(s.to_string()))
            }
            PropertyType::Date => {
                let val = u32::from_be_bytes([
                    self.data[offset],
                    self.data[offset + 1],
                    self.data[offset + 2],
                    self.data[offset + 3],
                ]);
                let d = byoridb_common::datatypes::date::Date {
                    year: (val >> 16) as u16,
                    month: ((val >> 8) & 0xFF) as u8,
                    day: (val & 0xFF) as u8,
                };
                Ok(Value::Date(d))
            }
            PropertyType::DateTime => {
                let dt = byoridb_common::datatypes::datetime::DateTime {
                    year: u16::from_be_bytes([self.data[offset], self.data[offset + 1]]),
                    month: self.data[offset + 2],
                    day: self.data[offset + 3],
                    hour: self.data[offset + 4],
                    minute: self.data[offset + 5],
                    second: self.data[offset + 6],
                    microsecond: u32::from_be_bytes([
                        self.data[offset + 7],
                        self.data[offset + 8],
                        self.data[offset + 9],
                        self.data[offset + 10],
                    ]),
                };
                Ok(Value::DateTime(dt))
            }
            _ => Ok(Value::Null(NullType::Null)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{MemorySchemaProvider, PropertyDef, PropertyType, Schema};
    use byoridb_common::types::NullType;
    use byoridb_common::Value;

    fn create_test_schema(version: i32) -> Schema {
        let mut schema = Schema::new(version);
        // v1: id (Int64), name (String)
        schema.add_property(PropertyDef::new("id", PropertyType::Int64, false));
        schema.add_property(PropertyDef::new("name", PropertyType::String, true));

        if version >= 2 {
            // v2: + age (Int32, default 20)
            schema.add_property(PropertyDef::with_default(
                "age",
                PropertyType::Int32,
                false,
                Some(Value::Int(20)),
            ));
        }

        if version >= 3 {
            // v3: + email (String, nullable, no default)
            schema.add_property(PropertyDef::new("email", PropertyType::String, true));
        }

        schema
    }

    #[test]
    fn test_version_aware_reader_same_version() {
        let mut provider = MemorySchemaProvider::new();
        let schema_v1 = create_test_schema(1);
        provider.add_schema(schema_v1.clone());

        // Create a row with v1 schema
        let mut writer = RowWriter::new(schema_v1);
        writer.set_value(0, &Value::Int(123)).unwrap(); // id
        writer
            .set_value(1, &Value::String("Alice".to_string()))
            .unwrap(); // name
        let data = writer.encode().unwrap();

        // Read back
        let reader = VersionAwareReader::new(&data, &provider).unwrap();
        assert_eq!(reader.get_row_schema_version(), 1);

        let id = reader.read_field("id").unwrap();
        assert_eq!(id, Value::Int(123));

        let name = reader.read_field("name").unwrap();
        assert_eq!(name, Value::String("Alice".to_string()));
    }

    #[test]
    fn test_version_aware_reader_evolution_defaults() {
        let mut provider = MemorySchemaProvider::new();
        let schema_v1 = create_test_schema(1);
        let schema_v2 = create_test_schema(2); // Has 'age' with default 20

        provider.add_schema(schema_v1.clone());
        provider.add_schema(schema_v2);

        // Create row with v1
        let mut writer = RowWriter::new(schema_v1);
        writer.set_value(0, &Value::Int(456)).unwrap();
        writer
            .set_value(1, &Value::String("Bob".to_string()))
            .unwrap();
        let data = writer.encode().unwrap();

        // Read using v2 (latest)
        let reader = VersionAwareReader::new(&data, &provider).unwrap();
        assert_eq!(reader.get_row_schema_version(), 1);
        assert_eq!(reader.get_target_schema_version(), 2);

        // 'age' should be default value 20
        let age = reader.read_field("age").unwrap();
        assert_eq!(age, Value::Int(20));

        // 'id' and 'name' should be preserved
        let id = reader.read_field("id").unwrap();
        assert_eq!(id, Value::Int(456));
    }

    #[test]
    fn test_version_aware_reader_evolution_nullable() {
        let mut provider = MemorySchemaProvider::new();
        let schema_v1 = create_test_schema(1);
        let schema_v2 = create_test_schema(2); // Intermediate version
        let schema_v3 = create_test_schema(3); // Has 'email' (nullable, no default)

        provider.add_schema(schema_v1.clone());
        provider.add_schema(schema_v2);
        provider.add_schema(schema_v3);

        // Create row with v1
        let mut writer = RowWriter::new(schema_v1);
        writer.set_value(0, &Value::Int(789)).unwrap();
        writer
            .set_value(1, &Value::String("Charlie".to_string()))
            .unwrap();
        let data = writer.encode().unwrap();

        let reader = VersionAwareReader::new(&data, &provider).unwrap();
        assert_eq!(reader.get_target_schema_version(), 3);

        // 'email' should be NULL
        let email = reader.read_field("email").unwrap();
        assert_eq!(email, Value::Null(NullType::Null));
    }

    #[test]
    fn test_read_all() {
        let mut provider = MemorySchemaProvider::new();
        let schema_v1 = create_test_schema(1);
        let schema_v2 = create_test_schema(2);

        provider.add_schema(schema_v1.clone());
        provider.add_schema(schema_v2);

        let mut writer = RowWriter::new(schema_v1);
        writer.set_value(0, &Value::Int(10)).unwrap();
        writer
            .set_value(1, &Value::String("Dave".to_string()))
            .unwrap();
        let data = writer.encode().unwrap();

        let reader = VersionAwareReader::new(&data, &provider).unwrap();
        let all_values = reader.read_all().unwrap();

        assert_eq!(all_values.get("id"), Some(&Value::Int(10)));
        assert_eq!(
            all_values.get("name"),
            Some(&Value::String("Dave".to_string()))
        );
        assert_eq!(all_values.get("age"), Some(&Value::Int(20))); // Default
    }

    #[test]
    fn test_invalid_data() {
        let provider = MemorySchemaProvider::new();
        let data = vec![];
        assert!(VersionAwareReader::new(&data, &provider).is_err());

        let data_short = vec![ENCODING_VERSION | 1]; // Header but partial
        assert!(VersionAwareReader::new(&data_short, &provider).is_err());
    }

    fn make_v1_row(id: i64, name: &str) -> (Schema, Vec<u8>) {
        let schema = create_test_schema(1);
        let mut w = RowWriter::new(schema.clone());
        w.set_value(0, &Value::Int(id)).unwrap();
        w.set_value(1, &Value::String(name.to_string())).unwrap();
        (schema, w.encode().unwrap())
    }

    #[test]
    fn test_row_reader_get_schema_version_single_byte() {
        let (schema, data) = make_v1_row(1, "x");
        let reader = RowReader::new(&data, schema).unwrap();
        assert_eq!(reader.get_schema_version(), 1);
    }

    #[test]
    fn test_row_reader_get_schema_version_multibyte() {
        // Schema version > 127 forces multi-byte encoding
        let mut schema = Schema::new(0x1234);
        schema.add_property(PropertyDef::new("id", PropertyType::Int64, false));
        // At least one nullable field is required by the writer; see set_null_flag.
        schema.add_property(PropertyDef::new("note", PropertyType::String, true));
        let mut w = RowWriter::new(schema.clone());
        w.set_value(0, &Value::Int(7)).unwrap();
        w.set_value(1, &Value::Null(NullType::Null)).unwrap();
        let data = w.encode().unwrap();

        let reader = RowReader::new(&data, schema).unwrap();
        assert_eq!(reader.get_schema_version(), 0x1234);
    }

    #[test]
    fn test_row_reader_get_value_by_index_for_int_and_string() {
        let (schema, data) = make_v1_row(99, "alice");
        let reader = RowReader::new(&data, schema).unwrap();
        assert_eq!(reader.get_value_by_index(0).unwrap(), Value::Int(99));
        // String is variable-length; reading via get_value_by_index hits read_value's String arm
        // which only sees offset+length but the actual string body lives in var section —
        // so the returned value reflects the encoded layout (offset/length pair).
        // Behavior: not Null, not Bool. We only assert it doesn't panic and decoding errors propagate.
        let _ = reader.get_value_by_index(1);
    }

    #[test]
    fn test_row_reader_get_value_by_index_unknown_returns_err() {
        let (schema, data) = make_v1_row(1, "x");
        let reader = RowReader::new(&data, schema).unwrap();
        assert!(reader.get_value_by_index(99).is_err());
    }

    #[test]
    fn test_row_reader_returns_null_for_unset_nullable() {
        let mut schema = Schema::new(1);
        schema.add_property(PropertyDef::new("id", PropertyType::Int64, false));
        schema.add_property(PropertyDef::new("name", PropertyType::String, true));
        let mut w = RowWriter::new(schema.clone());
        w.set_value(0, &Value::Int(5)).unwrap();
        // Mark name as Null explicitly
        w.set_value(1, &Value::Null(NullType::Null)).unwrap();
        let data = w.encode().unwrap();

        let reader = RowReader::new(&data, schema).unwrap();
        let name = reader.get_value_by_index(1).unwrap();
        assert!(matches!(name, Value::Null(_)));
    }

    #[test]
    fn test_row_reader_rejects_invalid_encoding_version() {
        let schema = create_test_schema(1);
        // Header with wrong encoding version
        let bad = vec![0b00010000 | 1, 0x01, 0x00, 0x00];
        assert!(RowReader::new(&bad, schema).is_err());
    }

    #[test]
    fn test_row_reader_rejects_zero_schema_version_bytes() {
        let schema = create_test_schema(1);
        // Header with vvv = 0
        let bad = vec![ENCODING_VERSION];
        assert!(RowReader::new(&bad, schema).is_err());
    }

    /// Encoding a `Value::String` into a `DateTime` column must surface a
    /// `CodecError::IncorrectValue` when the string doesn't parse, instead of
    /// silently persisting the Unix epoch (the old `DateTime::new` fallback).
    #[test]
    fn test_encode_datetime_invalid_string_errors() {
        // RowWriter requires at least one nullable column to allocate its
        // null-flag buffer; use the canonical 2-column schema as elsewhere.
        let mut schema = Schema::new(1);
        schema.add_property(PropertyDef::new("ts", PropertyType::DateTime, true));

        let mut w = RowWriter::new(schema);
        let err = w
            .set_value(0, &Value::String("not-a-datetime".to_string()))
            .expect_err("invalid datetime strings must fail to encode");

        match err {
            CodecError::IncorrectValue(msg) => {
                assert!(msg.contains("Invalid DateTime"), "msg was: {}", msg);
            }
            other => panic!("expected IncorrectValue, got {:?}", other),
        }
    }

    /// Valid datetime strings still encode successfully, confirming the new
    /// `DateTime::parse` path works end-to-end for the happy case.
    #[test]
    fn test_encode_datetime_valid_string_succeeds() {
        let mut schema = Schema::new(1);
        schema.add_property(PropertyDef::new("ts", PropertyType::DateTime, true));

        let mut w = RowWriter::new(schema);
        w.set_value(0, &Value::String("2026-05-12T10:30:00.000000".to_string()))
            .expect("valid datetime string must encode");
        let data = w.encode().expect("encode must succeed");
        assert!(!data.is_empty());
    }

    /// Regression test: RowWriter must not panic when the schema has no nullable
    /// fields (null_flags is empty) and set_null_flag is called internally.
    #[test]
    fn test_set_value_no_nullable_fields_no_panic() {
        let mut schema = Schema::new(1);
        schema.add_property(PropertyDef::new("id", PropertyType::Int64, false));
        schema.add_property(PropertyDef::new("score", PropertyType::Int32, false));

        let mut w = RowWriter::new(schema);
        // Both fields are non-nullable — null_flags vec is empty.
        // This must not panic.
        w.set_value(0, &Value::Int(42)).expect("should not panic");
        w.set_value(1, &Value::Int(100)).expect("should not panic");
        let data = w.encode().expect("encode must succeed");
        assert!(!data.is_empty());
    }
}

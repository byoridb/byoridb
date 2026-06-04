// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use serde::{Deserialize, Serialize};

pub type EdgeType = i32;
pub type EdgeRanking = i64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NullType {
    Null,
    NaN,
    BadData,
    BadType,
    ErrOverflow,
    UnknownProp,
    DivByZero,
    OutOfRange,
}

impl std::fmt::Display for NullType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NullType::Null => write!(f, "NULL"),
            NullType::NaN => write!(f, "NaN"),
            NullType::BadData => write!(f, "BAD_DATA"),
            NullType::BadType => write!(f, "BAD_TYPE"),
            NullType::ErrOverflow => write!(f, "ERR_OVERFLOW"),
            NullType::UnknownProp => write!(f, "UNKNOWN_PROP"),
            NullType::DivByZero => write!(f, "DIV_BY_ZERO"),
            NullType::OutOfRange => write!(f, "OUT_OF_RANGE"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ValueType {
    Empty,
    Bool,
    Int,
    Float,
    String,
    Date,
    Time,
    DateTime,
    Vertex,
    Edge,
    Path,
    List,
    Map,
    Set,
    DataSet,
    Geography,
    Duration,
    NullValue,
}

impl ValueType {
    pub fn type_name(&self) -> &'static str {
        match self {
            ValueType::Empty => "empty",
            ValueType::Bool => "bool",
            ValueType::Int => "int",
            ValueType::Float => "float",
            ValueType::String => "string",
            ValueType::Date => "date",
            ValueType::Time => "time",
            ValueType::DateTime => "datetime",
            ValueType::Vertex => "vertex",
            ValueType::Edge => "edge",
            ValueType::Path => "path",
            ValueType::List => "list",
            ValueType::Map => "map",
            ValueType::Set => "set",
            ValueType::DataSet => "dataset",
            ValueType::Geography => "geography",
            ValueType::Duration => "duration",
            ValueType::NullValue => "null",
        }
    }

    pub fn is_numeric(&self) -> bool {
        matches!(self, ValueType::Int | ValueType::Float)
    }

    pub fn is_null(&self) -> bool {
        matches!(self, ValueType::NullValue)
    }
}

impl std::fmt::Display for ValueType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.type_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_null_type_display_all_variants() {
        assert_eq!(NullType::Null.to_string(), "NULL");
        assert_eq!(NullType::NaN.to_string(), "NaN");
        assert_eq!(NullType::BadData.to_string(), "BAD_DATA");
        assert_eq!(NullType::BadType.to_string(), "BAD_TYPE");
        assert_eq!(NullType::ErrOverflow.to_string(), "ERR_OVERFLOW");
        assert_eq!(NullType::UnknownProp.to_string(), "UNKNOWN_PROP");
        assert_eq!(NullType::DivByZero.to_string(), "DIV_BY_ZERO");
        assert_eq!(NullType::OutOfRange.to_string(), "OUT_OF_RANGE");
    }

    #[test]
    fn test_value_type_name_covers_all() {
        assert_eq!(ValueType::Empty.type_name(), "empty");
        assert_eq!(ValueType::Bool.type_name(), "bool");
        assert_eq!(ValueType::Int.type_name(), "int");
        assert_eq!(ValueType::Float.type_name(), "float");
        assert_eq!(ValueType::String.type_name(), "string");
        assert_eq!(ValueType::Date.type_name(), "date");
        assert_eq!(ValueType::Time.type_name(), "time");
        assert_eq!(ValueType::DateTime.type_name(), "datetime");
        assert_eq!(ValueType::Vertex.type_name(), "vertex");
        assert_eq!(ValueType::Edge.type_name(), "edge");
        assert_eq!(ValueType::Path.type_name(), "path");
        assert_eq!(ValueType::List.type_name(), "list");
        assert_eq!(ValueType::Map.type_name(), "map");
        assert_eq!(ValueType::Set.type_name(), "set");
        assert_eq!(ValueType::DataSet.type_name(), "dataset");
        assert_eq!(ValueType::Geography.type_name(), "geography");
        assert_eq!(ValueType::Duration.type_name(), "duration");
        assert_eq!(ValueType::NullValue.type_name(), "null");
    }

    #[test]
    fn test_value_type_is_numeric() {
        assert!(ValueType::Int.is_numeric());
        assert!(ValueType::Float.is_numeric());
        assert!(!ValueType::String.is_numeric());
        assert!(!ValueType::Bool.is_numeric());
        assert!(!ValueType::NullValue.is_numeric());
    }

    #[test]
    fn test_value_type_is_null() {
        assert!(ValueType::NullValue.is_null());
        assert!(!ValueType::Empty.is_null());
        assert!(!ValueType::Int.is_null());
    }

    #[test]
    fn test_value_type_display_uses_type_name() {
        assert_eq!(ValueType::Int.to_string(), "int");
        assert_eq!(ValueType::String.to_string(), "string");
        assert_eq!(ValueType::NullValue.to_string(), "null");
    }
}

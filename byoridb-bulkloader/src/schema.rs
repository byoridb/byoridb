// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Reads tag/edge schemas from the store (written by `CREATE TAG/EDGE`) and
//! turns raw CSV cells into typed [`Value`]s.
//!
//! The loader does not require a schema — when a column has no declared type it
//! falls back to keeping the cell as a string (the engine's INSERT path does not
//! coerce types either). When a schema *is* present, declared types let queries
//! like `WHERE price > 10` work against the loaded data.

use byoridb_common::types::NullType;
use byoridb_common::Value;
use std::collections::HashMap;

/// Map of `column name -> declared type tag` for one tag/edge, derived from the
/// schema JSON's `properties[].data_type`. The type tag is the parser
/// `DataType` serde representation (e.g. `"Int64"`, `"String"`, or for
/// `FixedString(n)` the object key `"FixedString"`).
pub type ColumnTypes = HashMap<String, String>;

/// Parse the `properties` array of a tag/edge schema JSON into column types.
/// Returns an empty map if the JSON has no usable `properties` (loader then
/// treats every column as a string).
pub fn column_types_from_schema(schema_json: &serde_json::Value) -> ColumnTypes {
    let mut out = ColumnTypes::new();
    let Some(props) = schema_json.get("properties").and_then(|p| p.as_array()) else {
        return out;
    };
    for p in props {
        let Some(name) = p.get("name").and_then(|n| n.as_str()) else {
            continue;
        };
        if let Some(ty) = data_type_tag(p.get("data_type")) {
            out.insert(name.to_string(), ty);
        }
    }
    out
}

/// Normalize a `DataType` serde value to its variant name. Unit variants
/// serialize as a bare string (`"Int64"`); `FixedString(usize)` serializes as
/// an object (`{"FixedString": 30}`) — we take the key.
fn data_type_tag(dt: Option<&serde_json::Value>) -> Option<String> {
    match dt? {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Object(o) => o.keys().next().cloned(),
        _ => None,
    }
}

/// Convert a raw CSV cell into a typed [`Value`] using the declared column type.
/// An empty cell becomes `Null`. If parsing fails for the declared type, the
/// cell is kept as a string rather than dropped (loader is lenient, like INSERT).
pub fn cell_to_value(raw: &str, declared: Option<&str>) -> Value {
    if raw.is_empty() {
        return Value::Null(NullType::Null);
    }
    match declared {
        Some("Bool") => raw
            .parse::<bool>()
            .map(Value::Bool)
            .unwrap_or_else(|_| Value::String(raw.to_string())),
        Some("Int8" | "Int16" | "Int32" | "Int64" | "Timestamp") => raw
            .parse::<i64>()
            .map(Value::Int)
            .unwrap_or_else(|_| Value::String(raw.to_string())),
        Some("Float" | "Double") => raw
            .parse::<f64>()
            .map(Value::Float)
            .unwrap_or_else(|_| Value::String(raw.to_string())),
        // String/FixedString/Date/Time/DateTime/Geography and unknown/None:
        // keep as string. The engine does not coerce these on INSERT either,
        // and date/geo parsing is out of scope for the first cut.
        _ => Value::String(raw.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_types_parses_bare_and_object_datatypes() {
        let json = serde_json::json!({
            "name": "sku",
            "properties": [
                {"name": "price", "data_type": "Float", "nullable": true},
                {"name": "code", "data_type": {"FixedString": 30}, "nullable": true},
                {"name": "qty", "data_type": "Int64", "nullable": true},
            ]
        });
        let ct = column_types_from_schema(&json);
        assert_eq!(ct.get("price").map(String::as_str), Some("Float"));
        assert_eq!(ct.get("code").map(String::as_str), Some("FixedString"));
        assert_eq!(ct.get("qty").map(String::as_str), Some("Int64"));
    }

    #[test]
    fn cell_conversion_respects_declared_type() {
        assert!(matches!(cell_to_value("42", Some("Int64")), Value::Int(42)));
        assert!(matches!(
            cell_to_value("1.5", Some("Double")),
            Value::Float(_)
        ));
        assert!(matches!(
            cell_to_value("true", Some("Bool")),
            Value::Bool(true)
        ));
        assert!(matches!(cell_to_value("abc", None), Value::String(_)));
        assert!(matches!(cell_to_value("", Some("Int64")), Value::Null(_)));
        // Unparseable-for-type falls back to string, not dropped.
        assert!(matches!(
            cell_to_value("x", Some("Int64")),
            Value::String(_)
        ));
    }
}

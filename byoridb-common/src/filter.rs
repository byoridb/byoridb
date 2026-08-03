// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Filter expressions for predicate pushdown
//!
//! These expressions are serializable and can be evaluated at the storage layer,
//! reducing network I/O by filtering data before transfer.

use crate::Value;
use serde::{Deserialize, Serialize};

/// Comparison operators for filter expressions
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CompareOp {
    Eq,       // ==
    Ne,       // !=
    Lt,       // <
    Le,       // <=
    Gt,       // >
    Ge,       // >=
    Contains, // CONTAINS (for strings)
    StartsWith,
    EndsWith,
    In, // IN list
}

/// Logical operators for combining filter expressions
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum LogicalOp {
    And,
    Or,
    Not,
}

/// A filter expression that can be evaluated against a row/vertex/edge
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum FilterExpr {
    /// Always true
    #[default]
    True,
    /// Always false
    False,
    /// Compare a field to a constant value
    Compare {
        field: String,
        op: CompareOp,
        value: Value,
    },
    /// Field IN (value1, value2, ...)
    InList { field: String, values: Vec<Value> },
    /// Field IS NULL
    IsNull { field: String },
    /// Field IS NOT NULL
    IsNotNull { field: String },
    /// Combine two expressions with AND
    And(Box<FilterExpr>, Box<FilterExpr>),
    /// Combine two expressions with OR
    Or(Box<FilterExpr>, Box<FilterExpr>),
    /// Negate an expression
    Not(Box<FilterExpr>),
}

impl FilterExpr {
    /// Create a simple equality filter
    pub fn eq(field: impl Into<String>, value: Value) -> Self {
        FilterExpr::Compare {
            field: field.into(),
            op: CompareOp::Eq,
            value,
        }
    }

    /// Create a not-equal filter
    pub fn ne(field: impl Into<String>, value: Value) -> Self {
        FilterExpr::Compare {
            field: field.into(),
            op: CompareOp::Ne,
            value,
        }
    }

    /// Create a less-than filter
    pub fn lt(field: impl Into<String>, value: Value) -> Self {
        FilterExpr::Compare {
            field: field.into(),
            op: CompareOp::Lt,
            value,
        }
    }

    /// Create a less-than-or-equal filter
    pub fn le(field: impl Into<String>, value: Value) -> Self {
        FilterExpr::Compare {
            field: field.into(),
            op: CompareOp::Le,
            value,
        }
    }

    /// Create a greater-than filter
    pub fn gt(field: impl Into<String>, value: Value) -> Self {
        FilterExpr::Compare {
            field: field.into(),
            op: CompareOp::Gt,
            value,
        }
    }

    /// Create a greater-than-or-equal filter
    pub fn ge(field: impl Into<String>, value: Value) -> Self {
        FilterExpr::Compare {
            field: field.into(),
            op: CompareOp::Ge,
            value,
        }
    }

    /// Create an IN list filter
    pub fn in_list(field: impl Into<String>, values: Vec<Value>) -> Self {
        FilterExpr::InList {
            field: field.into(),
            values,
        }
    }

    /// Create IS NULL filter
    pub fn is_null(field: impl Into<String>) -> Self {
        FilterExpr::IsNull {
            field: field.into(),
        }
    }

    /// Create IS NOT NULL filter
    pub fn is_not_null(field: impl Into<String>) -> Self {
        FilterExpr::IsNotNull {
            field: field.into(),
        }
    }

    /// Combine with AND
    pub fn and(self, other: FilterExpr) -> Self {
        FilterExpr::And(Box::new(self), Box::new(other))
    }

    /// Combine with OR
    pub fn or(self, other: FilterExpr) -> Self {
        FilterExpr::Or(Box::new(self), Box::new(other))
    }

    /// Negate the expression
    pub fn not(self) -> Self {
        FilterExpr::Not(Box::new(self))
    }

    /// Evaluate the filter against a set of field values
    /// Returns true if the row passes the filter
    pub fn evaluate(&self, get_field: &impl Fn(&str) -> Option<Value>) -> bool {
        match self {
            FilterExpr::True => true,
            FilterExpr::False => false,

            FilterExpr::Compare { field, op, value } => {
                if let Some(field_value) = get_field(field) {
                    Self::compare_values(&field_value, op, value)
                } else {
                    false // Field not found, filter fails
                }
            }

            FilterExpr::InList { field, values } => {
                if let Some(field_value) = get_field(field) {
                    values.contains(&field_value)
                } else {
                    false
                }
            }

            FilterExpr::IsNull { field } => get_field(field).is_none_or(|v| v.is_null()),

            FilterExpr::IsNotNull { field } => get_field(field).is_some_and(|v| !v.is_null()),

            FilterExpr::And(left, right) => left.evaluate(get_field) && right.evaluate(get_field),

            FilterExpr::Or(left, right) => left.evaluate(get_field) || right.evaluate(get_field),

            FilterExpr::Not(expr) => !expr.evaluate(get_field),
        }
    }

    /// Compare two values with the given operator
    fn compare_values(left: &Value, op: &CompareOp, right: &Value) -> bool {
        match op {
            CompareOp::Eq => left == right,
            CompareOp::Ne => left != right,

            CompareOp::Lt => Self::compare_lt(left, right),
            CompareOp::Le => Self::compare_le(left, right),
            CompareOp::Gt => Self::compare_gt(left, right),
            CompareOp::Ge => Self::compare_ge(left, right),

            CompareOp::Contains => {
                if let (Value::String(s1), Value::String(s2)) = (left, right) {
                    s1.contains(s2.as_str())
                } else {
                    false
                }
            }

            CompareOp::StartsWith => {
                if let (Value::String(s1), Value::String(s2)) = (left, right) {
                    s1.starts_with(s2.as_str())
                } else {
                    false
                }
            }

            CompareOp::EndsWith => {
                if let (Value::String(s1), Value::String(s2)) = (left, right) {
                    s1.ends_with(s2.as_str())
                } else {
                    false
                }
            }

            CompareOp::In => false, // Handled by InList variant
        }
    }

    /// Compare less-than
    fn compare_lt(left: &Value, right: &Value) -> bool {
        match (left, right) {
            (Value::Int(a), Value::Int(b)) => a < b,
            (Value::Float(a), Value::Float(b)) => a < b,
            (Value::String(a), Value::String(b)) => a < b,
            (Value::Bool(a), Value::Bool(b)) => a < b,
            (Value::Int(a), Value::Float(b)) => (*a as f64) < *b,
            (Value::Float(a), Value::Int(b)) => *a < (*b as f64),
            _ => false,
        }
    }

    /// Compare less-than-or-equal
    fn compare_le(left: &Value, right: &Value) -> bool {
        match (left, right) {
            (Value::Int(a), Value::Int(b)) => a <= b,
            (Value::Float(a), Value::Float(b)) => a <= b,
            (Value::String(a), Value::String(b)) => a <= b,
            (Value::Bool(a), Value::Bool(b)) => a <= b,
            (Value::Int(a), Value::Float(b)) => (*a as f64) <= *b,
            (Value::Float(a), Value::Int(b)) => *a <= (*b as f64),
            _ => false,
        }
    }

    /// Compare greater-than
    fn compare_gt(left: &Value, right: &Value) -> bool {
        match (left, right) {
            (Value::Int(a), Value::Int(b)) => a > b,
            (Value::Float(a), Value::Float(b)) => a > b,
            (Value::String(a), Value::String(b)) => a > b,
            (Value::Bool(a), Value::Bool(b)) => a > b,
            (Value::Int(a), Value::Float(b)) => (*a as f64) > *b,
            (Value::Float(a), Value::Int(b)) => *a > (*b as f64),
            _ => false,
        }
    }

    /// Compare greater-than-or-equal
    fn compare_ge(left: &Value, right: &Value) -> bool {
        match (left, right) {
            (Value::Int(a), Value::Int(b)) => a >= b,
            (Value::Float(a), Value::Float(b)) => a >= b,
            (Value::String(a), Value::String(b)) => a >= b,
            (Value::Bool(a), Value::Bool(b)) => a >= b,
            (Value::Int(a), Value::Float(b)) => (*a as f64) >= *b,
            (Value::Float(a), Value::Int(b)) => *a >= (*b as f64),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_eq_filter() {
        let filter = FilterExpr::eq("name", Value::String("Alice".into()));

        let get_field = |field: &str| -> Option<Value> {
            match field {
                "name" => Some(Value::String("Alice".into())),
                "age" => Some(Value::Int(30)),
                _ => None,
            }
        };

        assert!(filter.evaluate(&get_field));

        let filter2 = FilterExpr::eq("name", Value::String("Bob".into()));
        assert!(!filter2.evaluate(&get_field));
    }

    #[test]
    fn test_comparison_filters() {
        let get_field = |field: &str| -> Option<Value> {
            match field {
                "age" => Some(Value::Int(30)),
                _ => None,
            }
        };

        assert!(FilterExpr::gt("age", Value::Int(25)).evaluate(&get_field));
        assert!(FilterExpr::ge("age", Value::Int(30)).evaluate(&get_field));
        assert!(FilterExpr::lt("age", Value::Int(35)).evaluate(&get_field));
        assert!(FilterExpr::le("age", Value::Int(30)).evaluate(&get_field));
        assert!(!FilterExpr::lt("age", Value::Int(25)).evaluate(&get_field));
    }

    #[test]
    fn test_bool_comparison_filters() {
        let get_field = |field: &str| match field {
            "enabled" => Some(Value::Bool(true)),
            _ => None,
        };

        assert!(FilterExpr::gt("enabled", Value::Bool(false)).evaluate(&get_field));
        assert!(FilterExpr::ge("enabled", Value::Bool(true)).evaluate(&get_field));
        assert!(!FilterExpr::lt("enabled", Value::Bool(false)).evaluate(&get_field));
        assert!(FilterExpr::le("enabled", Value::Bool(true)).evaluate(&get_field));
    }

    #[test]
    fn test_and_or_filters() {
        let get_field = |field: &str| -> Option<Value> {
            match field {
                "name" => Some(Value::String("Alice".into())),
                "age" => Some(Value::Int(30)),
                _ => None,
            }
        };

        let filter = FilterExpr::eq("name", Value::String("Alice".into()))
            .and(FilterExpr::gt("age", Value::Int(25)));
        assert!(filter.evaluate(&get_field));

        let filter2 = FilterExpr::eq("name", Value::String("Bob".into()))
            .or(FilterExpr::gt("age", Value::Int(25)));
        assert!(filter2.evaluate(&get_field));
    }

    #[test]
    fn test_in_list_filter() {
        let get_field = |field: &str| -> Option<Value> {
            match field {
                "status" => Some(Value::String("active".into())),
                _ => None,
            }
        };

        let filter = FilterExpr::in_list(
            "status",
            vec![
                Value::String("active".into()),
                Value::String("pending".into()),
            ],
        );
        assert!(filter.evaluate(&get_field));

        let filter2 = FilterExpr::in_list("status", vec![Value::String("deleted".into())]);
        assert!(!filter2.evaluate(&get_field));
    }

    #[test]
    fn test_string_operations() {
        let get_field = |field: &str| -> Option<Value> {
            match field {
                "email" => Some(Value::String("alice@example.com".into())),
                _ => None,
            }
        };

        let filter = FilterExpr::Compare {
            field: "email".into(),
            op: CompareOp::Contains,
            value: Value::String("@example".into()),
        };
        assert!(filter.evaluate(&get_field));

        let filter2 = FilterExpr::Compare {
            field: "email".into(),
            op: CompareOp::StartsWith,
            value: Value::String("alice".into()),
        };
        assert!(filter2.evaluate(&get_field));

        let filter3 = FilterExpr::Compare {
            field: "email".into(),
            op: CompareOp::EndsWith,
            value: Value::String(".com".into()),
        };
        assert!(filter3.evaluate(&get_field));
    }
}

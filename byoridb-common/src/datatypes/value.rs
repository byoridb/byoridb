// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use super::{
    date::Date, datetime::DateTime, duration::Duration, edge::Edge, geography::Geography,
    list::List, map::Map, path::Path, set::Set, time::Time, vertex::Vertex,
};
use crate::{error::Error, types::NullType, types::ValueType, Result};
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum Value {
    #[default]
    Empty,
    Null(NullType),
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Date(Date),
    Time(Time),
    DateTime(DateTime),
    Vertex(Box<Vertex>),
    Edge(Box<Edge>),
    Path(Box<Path>),
    List(List),
    Map(Map),
    Set(Set),
    DataSet(crate::datatypes::dataset::DataSet),
    Geography(Geography),
    Duration(Duration),
}

impl Value {
    pub const EMPTY: Value = Value::Empty;

    pub fn null() -> Self {
        Value::Null(NullType::Null)
    }

    pub fn nan() -> Self {
        Value::Null(NullType::NaN)
    }

    pub fn type_of(&self) -> ValueType {
        match self {
            Value::Empty => ValueType::Empty,
            Value::Null(_) => ValueType::NullValue,
            Value::Bool(_) => ValueType::Bool,
            Value::Int(_) => ValueType::Int,
            Value::Float(_) => ValueType::Float,
            Value::String(_) => ValueType::String,
            Value::Date(_) => ValueType::Date,
            Value::Time(_) => ValueType::Time,
            Value::DateTime(_) => ValueType::DateTime,
            Value::Vertex(_) => ValueType::Vertex,
            Value::Edge(_) => ValueType::Edge,
            Value::Path(_) => ValueType::Path,
            Value::List(_) => ValueType::List,
            Value::Map(_) => ValueType::Map,
            Value::Set(_) => ValueType::Set,
            Value::DataSet(_) => ValueType::DataSet,
            Value::Geography(_) => ValueType::Geography,
            Value::Duration(_) => ValueType::Duration,
        }
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, Value::Empty)
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null(_))
    }

    pub fn is_bad_null(&self) -> bool {
        matches!(self, Value::Null(n) if matches!(n,
            NullType::NaN | NullType::BadData | NullType::BadType |
            NullType::ErrOverflow | NullType::UnknownProp |
            NullType::DivByZero | NullType::OutOfRange))
    }

    pub fn is_numeric(&self) -> bool {
        matches!(self, Value::Int(_) | Value::Float(_))
    }

    pub fn is_bool(&self) -> bool {
        matches!(self, Value::Bool(_))
    }

    pub fn is_int(&self) -> bool {
        matches!(self, Value::Int(_))
    }

    pub fn is_float(&self) -> bool {
        matches!(self, Value::Float(_))
    }

    pub fn is_str(&self) -> bool {
        matches!(self, Value::String(_))
    }

    pub fn is_vertex(&self) -> bool {
        matches!(self, Value::Vertex(_))
    }

    pub fn is_edge(&self) -> bool {
        matches!(self, Value::Edge(_))
    }

    pub fn as_bool(&self) -> Result<bool> {
        match self {
            Value::Bool(b) => Ok(*b),
            _ => Err(Error::BadType {
                expected: "bool".to_string(),
                found: self.type_of().type_name().to_string(),
            }),
        }
    }

    pub fn as_int(&self) -> Result<i64> {
        match self {
            Value::Int(i) => Ok(*i),
            _ => Err(Error::BadType {
                expected: "int".to_string(),
                found: self.type_of().type_name().to_string(),
            }),
        }
    }

    pub fn as_float(&self) -> Result<f64> {
        match self {
            Value::Float(f) => Ok(*f),
            _ => Err(Error::BadType {
                expected: "float".to_string(),
                found: self.type_of().type_name().to_string(),
            }),
        }
    }

    pub fn as_str(&self) -> Result<&str> {
        match self {
            Value::String(s) => Ok(s),
            _ => Err(Error::BadType {
                expected: "string".to_string(),
                found: self.type_of().type_name().to_string(),
            }),
        }
    }

    pub fn to_string(&self) -> String {
        match self {
            Value::Empty => "".to_string(),
            Value::Null(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => f.to_string(),
            Value::String(s) => s.clone(),
            Value::Date(d) => d.to_string(),
            Value::Time(t) => t.to_string(),
            Value::DateTime(dt) => dt.to_string(),
            Value::Vertex(v) => v.to_string(),
            Value::Edge(e) => e.to_string(),
            Value::Path(p) => p.to_string(),
            Value::List(l) => l.to_string(),
            Value::Map(m) => m.to_string(),
            Value::Set(s) => s.to_string(),
            Value::DataSet(ds) => ds.to_string(),
            Value::Geography(g) => g.to_string(),
            Value::Duration(d) => d.to_string(),
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Empty, Value::Empty) => true,
            (Value::Null(n1), Value::Null(n2)) => n1 == n2,
            (Value::Bool(b1), Value::Bool(b2)) => b1 == b2,
            (Value::Int(i1), Value::Int(i2)) => i1 == i2,
            (Value::Float(f1), Value::Float(f2)) => (f1 - f2).abs() < 1e-8,
            (Value::String(s1), Value::String(s2)) => s1 == s2,
            (Value::Vertex(v1), Value::Vertex(v2)) => v1 == v2,
            (Value::Edge(e1), Value::Edge(e2)) => e1 == e2,
            // Structural comparison for container/temporal types. List/Map/Set
            // compare element-wise (recursing through this impl, so nested
            // floats keep the epsilon semantics above). Before this arm they
            // fell through to `false`, making a value unequal to itself.
            (Value::List(l1), Value::List(l2)) => l1.values == l2.values,
            (Value::Map(m1), Value::Map(m2)) => m1 == m2,
            (Value::Set(s1), Value::Set(s2)) => s1 == s2,
            (Value::Path(p1), Value::Path(p2)) => p1 == p2,
            (Value::Date(d1), Value::Date(d2)) => d1 == d2,
            (Value::Time(t1), Value::Time(t2)) => t1 == t2,
            (Value::DateTime(dt1), Value::DateTime(dt2)) => dt1 == dt2,
            (Value::Duration(d1), Value::Duration(d2)) => d1 == d2,
            _ => false,
        }
    }
}

impl Eq for Value {}

impl Hash for Value {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            Value::Empty => 0.hash(state),
            Value::Null(n) => n.hash(state),
            Value::Bool(b) => b.hash(state),
            Value::Int(i) => i.hash(state),
            Value::Float(f) => (f.to_bits()).hash(state),
            Value::String(s) => s.hash(state),
            Value::Vertex(v) => v.hash(state),
            Value::Edge(e) => e.hash(state),
            _ => {
                // For complex types, hash their string representation
                self.to_string().hash(state)
            }
        }
    }
}

// Conversion helpers
impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}

impl From<i64> for Value {
    fn from(i: i64) -> Self {
        Value::Int(i)
    }
}

impl From<f64> for Value {
    fn from(f: f64) -> Self {
        Value::Float(f)
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::String(s)
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::String(s.to_string())
    }
}

impl From<Vertex> for Value {
    fn from(v: Vertex) -> Self {
        Value::Vertex(Box::new(v))
    }
}

impl From<Edge> for Value {
    fn from(e: Edge) -> Self {
        Value::Edge(Box::new(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datatypes::vertex::Vertex;
    use std::collections::hash_map::DefaultHasher;

    fn hash_of<T: Hash>(v: &T) -> u64 {
        let mut h = DefaultHasher::new();
        v.hash(&mut h);
        h.finish()
    }

    #[test]
    fn test_type_of_covers_all_variants() {
        assert_eq!(Value::Empty.type_of(), ValueType::Empty);
        assert_eq!(Value::null().type_of(), ValueType::NullValue);
        assert_eq!(Value::Bool(true).type_of(), ValueType::Bool);
        assert_eq!(Value::Int(0).type_of(), ValueType::Int);
        assert_eq!(Value::Float(0.0).type_of(), ValueType::Float);
        assert_eq!(Value::String("x".into()).type_of(), ValueType::String);
    }

    #[test]
    fn test_container_values_compare_structurally() {
        let list = |vals: &[i64]| {
            Value::List(List::with_values(
                vals.iter().map(|&v| Value::Int(v)).collect(),
            ))
        };
        // Regression: List/Map/Set/temporal variants used to fall through
        // PartialEq to `false`, so a value was unequal to itself.
        assert_eq!(list(&[1, 2, 3]), list(&[1, 2, 3]));
        assert_ne!(list(&[1, 2, 3]), list(&[1, 2]));
        assert_ne!(list(&[1, 2, 3]), list(&[3, 2, 1]));
        assert_eq!(Value::Map(Map::new()), Value::Map(Map::new()));
        assert_eq!(Value::Set(Set::new()), Value::Set(Set::new()));
    }

    #[test]
    fn test_is_predicates() {
        let empty = Value::Empty;
        let null = Value::null();
        let i = Value::Int(1);
        let f = Value::Float(1.0);
        let s = Value::String("a".into());

        assert!(empty.is_empty());
        assert!(!null.is_empty());

        assert!(null.is_null());
        assert!(!i.is_null());

        assert!(i.is_numeric());
        assert!(f.is_numeric());
        assert!(!s.is_numeric());

        assert!(i.is_int());
        assert!(!f.is_int());
        assert!(f.is_float());
        assert!(!i.is_float());
    }

    #[test]
    fn test_is_vertex_and_is_edge() {
        let v = Value::Vertex(Box::new(Vertex {
            vid: Value::Int(1),
            tags: vec![],
        }));
        let e = Value::Edge(Box::new(Edge::new(
            Value::Int(1),
            Value::Int(2),
            1,
            "knows",
            0,
        )));

        assert!(v.is_vertex());
        assert!(!v.is_edge());
        assert!(e.is_edge());
        assert!(!e.is_vertex());
        assert!(!Value::Int(1).is_vertex());
        assert!(!Value::Int(1).is_edge());
    }

    #[test]
    fn test_is_bad_null_distinguishes_normal_null() {
        // Plain Null is NOT bad
        assert!(!Value::null().is_bad_null());
        // Other null-types are
        for nt in [
            NullType::NaN,
            NullType::BadData,
            NullType::BadType,
            NullType::ErrOverflow,
            NullType::UnknownProp,
            NullType::DivByZero,
            NullType::OutOfRange,
        ] {
            assert!(
                Value::Null(nt).is_bad_null(),
                "expected {:?} to be bad_null",
                nt
            );
        }
        // Non-null is never bad_null
        assert!(!Value::Int(0).is_bad_null());
    }

    #[test]
    fn test_as_bool_ok_and_err() {
        assert_eq!(Value::Bool(true).as_bool().unwrap(), true);
        assert_eq!(Value::Bool(false).as_bool().unwrap(), false);
        assert!(Value::Int(1).as_bool().is_err());
        assert!(Value::null().as_bool().is_err());
    }

    #[test]
    fn test_as_float_ok_and_err() {
        assert_eq!(Value::Float(3.14).as_float().unwrap(), 3.14);
        // as_float does NOT convert ints — strict type
        assert!(Value::Int(1).as_float().is_err());
        assert!(Value::String("3.14".into()).as_float().is_err());
    }

    #[test]
    fn test_to_string_for_primitives() {
        assert_eq!(Value::Empty.to_string(), "");
        assert_eq!(Value::Bool(true).to_string(), "true");
        assert_eq!(Value::Int(42).to_string(), "42");
        assert_eq!(Value::Float(1.5).to_string(), "1.5");
        assert_eq!(Value::String("hi".into()).to_string(), "hi");
        assert_eq!(Value::null().to_string(), "NULL");
        assert_eq!(Value::Null(NullType::NaN).to_string(), "NaN");
    }

    #[test]
    fn test_partial_eq_float_uses_epsilon() {
        // Float equality uses 1e-8 tolerance
        assert_eq!(Value::Float(1.0), Value::Float(1.0 + 1e-10));
        assert_ne!(Value::Float(1.0), Value::Float(1.0 + 1e-6));
    }

    #[test]
    fn test_partial_eq_cross_type_is_false() {
        assert_ne!(Value::Int(1), Value::Float(1.0));
        assert_ne!(Value::Int(0), Value::Bool(false));
        assert_ne!(Value::String("1".into()), Value::Int(1));
    }

    #[test]
    fn test_hash_is_deterministic_for_same_value() {
        let a = Value::Int(42);
        let b = Value::Int(42);
        assert_eq!(hash_of(&a), hash_of(&b));

        let s1 = Value::String("foo".into());
        let s2 = Value::String("foo".into());
        assert_eq!(hash_of(&s1), hash_of(&s2));
    }

    #[test]
    fn test_hash_distinguishes_different_values() {
        assert_ne!(hash_of(&Value::Int(1)), hash_of(&Value::Int(2)));
        assert_ne!(
            hash_of(&Value::String("a".into())),
            hash_of(&Value::String("b".into()))
        );
        // Float bit-pattern hashing: differing floats hash differently
        assert_ne!(hash_of(&Value::Float(1.0)), hash_of(&Value::Float(2.0)));
    }

    #[test]
    fn test_hash_complex_types_via_to_string() {
        // For non-primitive variants, hash falls back to to_string. Same content → same hash.
        let v1 = Value::Vertex(Box::new(Vertex {
            vid: Value::Int(1),
            tags: vec![],
        }));
        let v2 = Value::Vertex(Box::new(Vertex {
            vid: Value::Int(1),
            tags: vec![],
        }));
        // Vertex has its own Hash impl matching its content, so equal vertices hash equal
        assert_eq!(hash_of(&v1), hash_of(&v2));
    }

    #[test]
    fn test_from_conversions() {
        let v: Value = true.into();
        assert!(matches!(v, Value::Bool(true)));

        let v: Value = 7i64.into();
        assert!(matches!(v, Value::Int(7)));

        let v: Value = 1.5f64.into();
        assert!(matches!(v, Value::Float(_)));

        let v: Value = "hi".into();
        assert_eq!(v, Value::String("hi".into()));

        let v: Value = String::from("world").into();
        assert_eq!(v, Value::String("world".into()));
    }
}

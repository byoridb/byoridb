// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use crate::datatypes::value::Value;
use crate::types::{EdgeRanking, EdgeType};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub src: Value,
    pub dst: Value,
    pub edge_type: EdgeType,
    pub name: String,
    pub ranking: EdgeRanking,
    pub props: HashMap<String, Value>,
}

impl Edge {
    pub fn new(
        src: Value,
        dst: Value,
        edge_type: EdgeType,
        name: impl Into<String>,
        ranking: EdgeRanking,
    ) -> Self {
        Edge {
            src,
            dst,
            edge_type,
            name: name.into(),
            ranking,
            props: HashMap::new(),
        }
    }

    pub fn with_props(
        src: Value,
        dst: Value,
        edge_type: EdgeType,
        name: impl Into<String>,
        ranking: EdgeRanking,
        props: HashMap<String, Value>,
    ) -> Self {
        Edge {
            src,
            dst,
            edge_type,
            name: name.into(),
            ranking,
            props,
        }
    }

    pub fn contains(&self, key: &str) -> bool {
        self.props.contains_key(key)
    }

    pub fn value(&self, key: &str) -> Option<&Value> {
        self.props.get(key)
    }

    pub fn key_equal(&self, other: &Edge) -> bool {
        self.src == other.src
            && self.dst == other.dst
            && self.edge_type == other.edge_type
            && self.ranking == other.ranking
    }

    pub fn id(&self) -> String {
        format!(
            "{}_{}_{}_{}",
            self.src.to_string(),
            self.edge_type,
            self.ranking,
            self.dst.to_string()
        )
    }

    pub fn format(&mut self) {
        if self.edge_type < 0 {
            self.reverse();
        }
    }

    pub fn reverse(&mut self) {
        std::mem::swap(&mut self.src, &mut self.dst);
        self.edge_type = -self.edge_type;
    }

    pub fn to_string(&self) -> String {
        format!(
            "{} -> [{} {} {} {}]{}",
            self.src.to_string(),
            self.name,
            self.edge_type,
            self.ranking,
            self.dst.to_string(),
            if self.props.is_empty() {
                String::new()
            } else {
                format!(" {:?}", self.props)
            }
        )
    }
}

impl PartialEq for Edge {
    fn eq(&self, other: &Self) -> bool {
        self.src == other.src
            && self.dst == other.dst
            && self.edge_type == other.edge_type
            && self.ranking == other.ranking
            && self.name == other.name
            && self.props == other.props
    }
}

impl Eq for Edge {}

impl Hash for Edge {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.src.hash(state);
        self.dst.hash(state);
        self.edge_type.hash(state);
        self.ranking.hash(state);
        self.name.hash(state);
        // Note: props not included in hash for performance
        for (k, v) in &self.props {
            k.hash(state);
            v.hash(state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::hash_map::DefaultHasher;

    fn make(src: i64, dst: i64, etype: EdgeType) -> Edge {
        Edge::new(Value::Int(src), Value::Int(dst), etype, "knows", 0)
    }

    fn hash_of(e: &Edge) -> u64 {
        let mut h = DefaultHasher::new();
        e.hash(&mut h);
        h.finish()
    }

    #[test]
    fn test_format_canonicalizes_negative_edge_type() {
        // Negative edge_type means reverse direction; format() should swap and flip sign.
        let mut e = make(1, 2, -7);
        e.format();
        assert_eq!(e.src, Value::Int(2));
        assert_eq!(e.dst, Value::Int(1));
        assert_eq!(e.edge_type, 7);
    }

    #[test]
    fn test_format_leaves_positive_edge_type_untouched() {
        let mut e = make(1, 2, 7);
        e.format();
        assert_eq!(e.src, Value::Int(1));
        assert_eq!(e.dst, Value::Int(2));
        assert_eq!(e.edge_type, 7);
    }

    #[test]
    fn test_to_string_includes_endpoints_and_name() {
        let e = make(1, 2, 7);
        let s = e.to_string();
        assert!(s.contains("knows"));
        assert!(s.contains("1"));
        assert!(s.contains("2"));
        assert!(s.contains("7"));
    }

    #[test]
    fn test_to_string_with_props_includes_props() {
        let mut props = HashMap::new();
        props.insert("since".to_string(), Value::Int(2020));
        let e = Edge::with_props(Value::Int(1), Value::Int(2), 1, "knows", 0, props);
        let s = e.to_string();
        assert!(s.contains("since"));
        assert!(s.contains("2020"));
    }

    #[test]
    fn test_hash_is_deterministic() {
        let e1 = make(1, 2, 7);
        let e2 = make(1, 2, 7);
        assert_eq!(hash_of(&e1), hash_of(&e2));
    }

    #[test]
    fn test_hash_distinguishes_endpoints() {
        let a = make(1, 2, 7);
        let b = make(2, 1, 7);
        assert_ne!(hash_of(&a), hash_of(&b));
    }

    #[test]
    fn test_id_is_stable_for_same_edge() {
        let e = make(1, 2, 7);
        assert_eq!(e.id(), e.id());
    }

    #[test]
    fn test_key_equal_ignores_name_and_props() {
        let mut a = make(1, 2, 7);
        let mut b = make(1, 2, 7);
        a.name = "knows".to_string();
        b.name = "loves".to_string();
        a.props.insert("x".into(), Value::Int(1));
        b.props.insert("x".into(), Value::Int(99));
        assert!(a.key_equal(&b));
    }
}

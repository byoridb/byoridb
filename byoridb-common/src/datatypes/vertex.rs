// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use crate::datatypes::value::Value;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Tag {
    pub name: String,
    pub props: HashMap<String, Value>,
}

impl Tag {
    pub fn new(name: impl Into<String>) -> Self {
        Tag {
            name: name.into(),
            props: HashMap::new(),
        }
    }

    pub fn with_props(name: impl Into<String>, props: HashMap<String, Value>) -> Self {
        Tag {
            name: name.into(),
            props,
        }
    }

    pub fn to_string(&self) -> String {
        format!("{}: {:?}", self.name, self.props)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vertex {
    pub vid: Value,
    pub tags: Vec<Tag>,
}

impl Vertex {
    pub fn new(vid: Value) -> Self {
        Vertex {
            vid,
            tags: Vec::new(),
        }
    }

    pub fn with_tags(vid: Value, tags: Vec<Tag>) -> Self {
        Vertex { vid, tags }
    }

    pub fn add_tag(&mut self, tag: Tag) {
        self.tags.push(tag);
    }

    pub fn contains(&self, key: &str) -> bool {
        self.tags.iter().any(|tag| tag.props.contains_key(key))
    }

    pub fn value(&self, key: &str) -> Option<&Value> {
        self.tags.iter().find_map(|tag| tag.props.get(key))
    }

    pub fn get_tag_prop(&self, tag_name: &str, prop: &str) -> Option<&Value> {
        self.tags
            .iter()
            .find(|tag| tag.name == tag_name)
            .and_then(|tag| tag.props.get(prop))
    }

    pub fn to_string(&self) -> String {
        format!("{}{}", self.vid.to_string(), self.tags_to_string())
    }

    fn tags_to_string(&self) -> String {
        if self.tags.is_empty() {
            "[]".to_string()
        } else {
            let tags: Vec<String> = self.tags.iter().map(|t| t.to_string()).collect();
            format!("[{}]", tags.join(", "))
        }
    }
}

impl PartialEq for Vertex {
    fn eq(&self, other: &Self) -> bool {
        self.vid == other.vid
    }
}

impl Eq for Vertex {}

impl Hash for Vertex {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.vid.hash(state);
    }
}

impl From<Value> for Vertex {
    fn from(vid: Value) -> Self {
        Vertex::new(vid)
    }
}

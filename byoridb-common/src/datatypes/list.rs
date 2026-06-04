// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use crate::datatypes::value::Value;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct List {
    pub values: Vec<Value>,
}

impl List {
    pub fn new() -> Self {
        List { values: Vec::new() }
    }

    pub fn with_values(values: Vec<Value>) -> Self {
        List { values }
    }

    pub fn add(&mut self, value: Value) {
        self.values.push(value);
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn to_string(&self) -> String {
        let values: Vec<String> = self.values.iter().map(|v| v.to_string()).collect();
        format!("[{}]", values.join(", "))
    }
}

impl Default for List {
    fn default() -> Self {
        Self::new()
    }
}

impl From<Vec<Value>> for List {
    fn from(values: Vec<Value>) -> Self {
        List { values }
    }
}

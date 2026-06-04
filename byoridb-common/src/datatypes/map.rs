// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Map {
    pub data: HashMap<String, crate::datatypes::value::Value>,
}

impl Map {
    pub fn new() -> Self {
        Map {
            data: HashMap::new(),
        }
    }

    pub fn to_string(&self) -> String {
        format!("{:?}", self.data)
    }
}

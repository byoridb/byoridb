// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Set {
    pub data: HashSet<String>,
}

impl Set {
    pub fn new() -> Self {
        Set {
            data: HashSet::new(),
        }
    }

    pub fn to_string(&self) -> String {
        format!("{:?}", self.data)
    }
}

// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Geography {
    pub value: String,
}

impl Geography {
    pub fn new(value: String) -> Self {
        Geography { value }
    }

    pub fn to_string(&self) -> String {
        self.value.clone()
    }
}

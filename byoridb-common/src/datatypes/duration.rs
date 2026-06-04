// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Duration {
    pub value: String,
}

impl Duration {
    pub fn new(value: String) -> Self {
        Duration { value }
    }

    pub fn to_string(&self) -> String {
        self.value.clone()
    }
}

// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Path {
    pub vertices: Vec<crate::datatypes::vertex::Vertex>,
}

impl Path {
    pub fn new() -> Self {
        Path {
            vertices: Vec::new(),
        }
    }

    pub fn to_string(&self) -> String {
        format!("{:?}", self.vertices)
    }
}

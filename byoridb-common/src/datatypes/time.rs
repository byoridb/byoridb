// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Time {
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub microsec: u32,
}

impl Time {
    pub fn new(hour: u8, minute: u8, second: u8, microsec: u32) -> Self {
        Time {
            hour,
            minute,
            second,
            microsec,
        }
    }

    pub fn to_string(&self) -> String {
        format!(
            "{:02}:{:02}:{:02}.{:06}",
            self.hour, self.minute, self.second, self.microsec
        )
    }
}

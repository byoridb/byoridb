// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

// Allow inherent to_string methods for data types (converting to Display would break API)
#![allow(clippy::inherent_to_string)]
// Allow new() without Default for types where default doesn't make semantic sense
#![allow(clippy::new_without_default)]
// Allow methods that could be confused with trait methods
#![allow(clippy::should_implement_trait)]

pub mod crypto;
pub mod datatypes;
pub mod error;
pub mod filter;
pub mod hash;
pub mod partition;
pub mod types;

pub use datatypes::{dataset::DataSet, edge::Edge, value::Value, vertex::Vertex};
pub use error::{Error, Result};
pub use filter::{CompareOp, FilterExpr};
pub use partition::PartitionStrategy;
pub use types::*;

// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Hash functions and consistent hash ring for partition routing
//!
//! This module provides:
//! - Centralized MurmurHash-like hash functions (used across all services)
//! - Consistent hash ring for minimal data movement during scaling
//!
//! # Usage
//!
//! ## Simple partition routing
//! ```
//! use byoridb_common::hash::{hash_vid, compute_partition};
//!
//! let vid = 12345i64;
//! let partition_num = 10;
//! let part_id = compute_partition(vid, partition_num);
//! ```
//!
//! ## Consistent hash ring
//! ```
//! use byoridb_common::hash::{ConsistentHashRing, RingConfig, RingNode};
//!
//! let mut ring = ConsistentHashRing::new(10, 2, RingConfig::default());
//! ring.add_node(RingNode::new("host1".to_string(), 9779));
//! ring.add_node(RingNode::new("host2".to_string(), 9779));
//!
//! let assignments = ring.get_all_assignments();
//! ```

pub mod consistent_ring;
pub mod murmur;

// Re-export commonly used items
pub use consistent_ring::{
    ConsistentHashRing, MigrationTask, PartitionAssignment, RingConfig, RingNode,
};
pub use murmur::{compute_partition, hash_bytes, hash_vid};

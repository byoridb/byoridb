// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Partition strategy definitions for data distribution
//!
//! This module provides different strategies for computing partition IDs from VIDs:
//! - Hash: Uses MurmurHash3 finalizer for uniform distribution
//! - Range: Maps VIDs to partitions based on boundary values
//! - Modulo: Simple modulo operation without hashing
//!
//! # Example
//!
//! ```
//! use byoridb_common::partition::PartitionStrategy;
//!
//! // Hash strategy (default)
//! let hash_strategy = PartitionStrategy::Hash;
//! let part_id = hash_strategy.compute_partition(12345, 10);
//! assert!(part_id >= 1 && part_id <= 10);
//!
//! // Range strategy
//! let range_strategy = PartitionStrategy::Range { boundaries: vec![100, 200, 300] };
//! let part_id = range_strategy.compute_partition(150, 4);
//! assert_eq!(part_id, 2); // 100 <= 150 < 200, so partition 2
//!
//! // Modulo strategy
//! let modulo_strategy = PartitionStrategy::Modulo;
//! let part_id = modulo_strategy.compute_partition(15, 10);
//! assert_eq!(part_id, 6); // 15 % 10 + 1 = 6
//! ```

use serde::{Deserialize, Serialize};
use std::fmt;

/// Partition strategy for distributing data across partitions
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PartitionStrategy {
    /// Hash-based partitioning using MurmurHash3 finalizer
    ///
    /// Formula: hash(vid) % partition_num + 1
    /// This is the default strategy providing uniform distribution.
    #[default]
    Hash,

    /// Range-based partitioning using boundary values
    ///
    /// VIDs are assigned to partitions based on their value relative to boundaries.
    /// - VID < boundaries[0] -> partition 1
    /// - boundaries[i-1] <= VID < boundaries[i] -> partition i+1
    /// - VID >= boundaries[n-1] -> partition n+1
    ///
    /// Note: The number of boundaries should be partition_num - 1.
    Range {
        /// Boundary values that divide the VID space
        boundaries: Vec<i64>,
    },

    /// Simple modulo-based partitioning without hashing
    ///
    /// Formula: vid % partition_num + 1
    /// Useful when VIDs are already uniformly distributed (e.g., auto-increment IDs).
    Modulo,
}

impl fmt::Display for PartitionStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PartitionStrategy::Hash => write!(f, "HASH"),
            PartitionStrategy::Range { boundaries } => {
                write!(f, "RANGE(")?;
                for (i, b) in boundaries.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", b)?;
                }
                write!(f, ")")
            }
            PartitionStrategy::Modulo => write!(f, "MODULO"),
        }
    }
}

impl PartitionStrategy {
    /// Compute the partition ID for a given VID
    ///
    /// # Arguments
    /// * `vid` - The vertex ID to partition
    /// * `partition_num` - The total number of partitions
    ///
    /// # Returns
    /// Partition ID in the range [1, partition_num]
    ///
    /// # Panics
    /// Does not panic; returns 1 for edge cases like partition_num = 0.
    pub fn compute_partition(&self, vid: i64, partition_num: u32) -> u32 {
        if partition_num == 0 {
            return 1;
        }

        match self {
            PartitionStrategy::Hash => crate::hash::compute_partition(vid, partition_num),

            PartitionStrategy::Range { boundaries } => {
                // Find the partition based on boundary values
                // Partition 1: vid < boundaries[0]
                // Partition 2: boundaries[0] <= vid < boundaries[1]
                // ...
                // Partition N: vid >= boundaries[N-2]
                for (i, &boundary) in boundaries.iter().enumerate() {
                    if vid < boundary {
                        return (i as u32) + 1;
                    }
                }
                // VID is >= all boundaries, assign to last partition
                (boundaries.len() as u32) + 1
            }

            PartitionStrategy::Modulo => {
                // Simple modulo without hashing
                let vid_abs = if vid < 0 {
                    (vid.wrapping_neg()) as u64
                } else {
                    vid as u64
                };
                (vid_abs % partition_num as u64) as u32 + 1
            }
        }
    }

    /// Validate the partition strategy configuration
    ///
    /// # Arguments
    /// * `partition_num` - The total number of partitions to validate against
    ///
    /// # Returns
    /// * `Ok(())` if the strategy is valid
    /// * `Err(String)` with a description of the validation error
    pub fn validate(&self, partition_num: u32) -> Result<(), String> {
        if partition_num == 0 {
            return Err("partition_num must be greater than 0".to_string());
        }

        match self {
            PartitionStrategy::Hash => Ok(()),

            PartitionStrategy::Range { boundaries } => {
                let expected_boundaries = partition_num.saturating_sub(1) as usize;

                if boundaries.len() != expected_boundaries {
                    return Err(format!(
                        "Range strategy requires {} boundaries for {} partitions, but got {}",
                        expected_boundaries,
                        partition_num,
                        boundaries.len()
                    ));
                }

                // Check that boundaries are sorted in ascending order
                for i in 1..boundaries.len() {
                    if boundaries[i] <= boundaries[i - 1] {
                        return Err(format!(
                            "Range boundaries must be strictly increasing: {} is not > {}",
                            boundaries[i],
                            boundaries[i - 1]
                        ));
                    }
                }

                Ok(())
            }

            PartitionStrategy::Modulo => Ok(()),
        }
    }

    /// Check if this is the default Hash strategy
    pub fn is_hash(&self) -> bool {
        matches!(self, PartitionStrategy::Hash)
    }

    /// Check if this is the Range strategy
    pub fn is_range(&self) -> bool {
        matches!(self, PartitionStrategy::Range { .. })
    }

    /// Check if this is the Modulo strategy
    pub fn is_modulo(&self) -> bool {
        matches!(self, PartitionStrategy::Modulo)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_strategy_basic() {
        let strategy = PartitionStrategy::Hash;

        // Same VID should always map to same partition
        let p1 = strategy.compute_partition(100, 10);
        let p2 = strategy.compute_partition(100, 10);
        assert_eq!(p1, p2);

        // Partition should be in range [1, partition_num]
        for vid in 0..1000 {
            let part = strategy.compute_partition(vid, 10);
            assert!(part >= 1 && part <= 10, "Partition {} out of range", part);
        }
    }

    #[test]
    fn test_hash_strategy_distribution() {
        let strategy = PartitionStrategy::Hash;
        let partition_num = 10u32;
        let mut counts = vec![0u32; partition_num as usize + 1];

        for vid in 0..10000 {
            let part = strategy.compute_partition(vid, partition_num);
            counts[part as usize] += 1;
        }

        // Each partition should have roughly 1000 VIDs (+-30%)
        for i in 1..=partition_num as usize {
            assert!(
                counts[i] > 700,
                "Partition {} has too few items: {}",
                i,
                counts[i]
            );
            assert!(
                counts[i] < 1300,
                "Partition {} has too many items: {}",
                i,
                counts[i]
            );
        }
    }

    #[test]
    fn test_range_strategy_basic() {
        let strategy = PartitionStrategy::Range {
            boundaries: vec![100, 200, 300],
        };

        // VID < 100 -> partition 1
        assert_eq!(strategy.compute_partition(50, 4), 1);
        assert_eq!(strategy.compute_partition(0, 4), 1);
        assert_eq!(strategy.compute_partition(-50, 4), 1);

        // 100 <= VID < 200 -> partition 2
        assert_eq!(strategy.compute_partition(100, 4), 2);
        assert_eq!(strategy.compute_partition(150, 4), 2);
        assert_eq!(strategy.compute_partition(199, 4), 2);

        // 200 <= VID < 300 -> partition 3
        assert_eq!(strategy.compute_partition(200, 4), 3);
        assert_eq!(strategy.compute_partition(250, 4), 3);
        assert_eq!(strategy.compute_partition(299, 4), 3);

        // VID >= 300 -> partition 4
        assert_eq!(strategy.compute_partition(300, 4), 4);
        assert_eq!(strategy.compute_partition(1000, 4), 4);
    }

    #[test]
    fn test_modulo_strategy_basic() {
        let strategy = PartitionStrategy::Modulo;

        // 15 % 10 + 1 = 6
        assert_eq!(strategy.compute_partition(15, 10), 6);

        // 0 % 10 + 1 = 1
        assert_eq!(strategy.compute_partition(0, 10), 1);

        // 10 % 10 + 1 = 1
        assert_eq!(strategy.compute_partition(10, 10), 1);

        // All partitions should be in range [1, partition_num]
        for vid in 0..1000 {
            let part = strategy.compute_partition(vid, 10);
            assert!(part >= 1 && part <= 10, "Partition {} out of range", part);
        }
    }

    #[test]
    fn test_modulo_strategy_negative_vid() {
        let strategy = PartitionStrategy::Modulo;

        // Negative VIDs should also produce valid partitions
        let part = strategy.compute_partition(-15, 10);
        assert!(part >= 1 && part <= 10);
    }

    #[test]
    fn test_validate_hash() {
        let strategy = PartitionStrategy::Hash;
        assert!(strategy.validate(10).is_ok());
        assert!(strategy.validate(1).is_ok());
        assert!(strategy.validate(0).is_err());
    }

    #[test]
    fn test_validate_range() {
        // Valid: 3 boundaries for 4 partitions
        let strategy = PartitionStrategy::Range {
            boundaries: vec![100, 200, 300],
        };
        assert!(strategy.validate(4).is_ok());

        // Invalid: wrong number of boundaries
        assert!(strategy.validate(3).is_err());
        assert!(strategy.validate(5).is_err());

        // Invalid: boundaries not sorted
        let invalid_strategy = PartitionStrategy::Range {
            boundaries: vec![200, 100, 300],
        };
        assert!(invalid_strategy.validate(4).is_err());

        // Invalid: duplicate boundaries
        let dup_strategy = PartitionStrategy::Range {
            boundaries: vec![100, 100, 200],
        };
        assert!(dup_strategy.validate(4).is_err());
    }

    #[test]
    fn test_validate_modulo() {
        let strategy = PartitionStrategy::Modulo;
        assert!(strategy.validate(10).is_ok());
        assert!(strategy.validate(1).is_ok());
        assert!(strategy.validate(0).is_err());
    }

    #[test]
    fn test_edge_cases() {
        let hash = PartitionStrategy::Hash;
        let modulo = PartitionStrategy::Modulo;

        // partition_num = 0 should return 1
        assert_eq!(hash.compute_partition(100, 0), 1);
        assert_eq!(modulo.compute_partition(100, 0), 1);

        // partition_num = 1 should always return 1
        assert_eq!(hash.compute_partition(100, 1), 1);
        assert_eq!(modulo.compute_partition(100, 1), 1);
    }

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", PartitionStrategy::Hash), "HASH");
        assert_eq!(format!("{}", PartitionStrategy::Modulo), "MODULO");
        assert_eq!(
            format!(
                "{}",
                PartitionStrategy::Range {
                    boundaries: vec![100, 200, 300]
                }
            ),
            "RANGE(100, 200, 300)"
        );
    }

    #[test]
    fn test_default() {
        assert_eq!(PartitionStrategy::default(), PartitionStrategy::Hash);
    }

    #[test]
    fn test_type_checks() {
        let hash = PartitionStrategy::Hash;
        let range = PartitionStrategy::Range {
            boundaries: vec![100],
        };
        let modulo = PartitionStrategy::Modulo;

        assert!(hash.is_hash());
        assert!(!hash.is_range());
        assert!(!hash.is_modulo());

        assert!(!range.is_hash());
        assert!(range.is_range());
        assert!(!range.is_modulo());

        assert!(!modulo.is_hash());
        assert!(!modulo.is_range());
        assert!(modulo.is_modulo());
    }
}

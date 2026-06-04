// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Centralized MurmurHash-like hash functions for partition routing
//!
//! This module provides the unified hash implementation used across all services:
//! - byoridb-meta: partition allocation
//! - byoridb-graph: partition routing
//! - byoridb-storage: partition validation
//!
//! Uses MurmurHash3 finalizer for good distribution properties.

/// Hash a VID (vertex ID) for partition routing
///
/// Uses MurmurHash3 finalizer which provides:
/// - Excellent avalanche properties
/// - Uniform distribution
/// - Fast computation
///
/// # Example
/// ```
/// use byoridb_common::hash::hash_vid;
///
/// let vid = 12345i64;
/// let hash = hash_vid(vid);
/// let partition = (hash % 10) as u32 + 1; // Partition 1-10
/// ```
#[inline]
pub fn hash_vid(vid: i64) -> u64 {
    let mut h = vid as u64;
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51afd7ed558ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ceb9fe1a85ec53);
    h ^= h >> 33;
    h
}

/// Hash arbitrary bytes for consistent hashing ring
///
/// Used for hashing node identifiers and partition IDs to positions on the ring.
///
/// # Example
/// ```
/// use byoridb_common::hash::hash_bytes;
///
/// let node_id = "host1:9779#42";
/// let position = hash_bytes(node_id.as_bytes());
/// ```
pub fn hash_bytes(data: &[u8]) -> u64 {
    let mut h: u64 = 0;
    for chunk in data.chunks(8) {
        let mut buf = [0u8; 8];
        buf[..chunk.len()].copy_from_slice(chunk);
        let v = u64::from_le_bytes(buf);
        h ^= v;
        h ^= h >> 33;
        h = h.wrapping_mul(0xff51afd7ed558ccd);
        h ^= h >> 33;
        h = h.wrapping_mul(0xc4ceb9fe1a85ec53);
    }
    h ^= h >> 33;
    h
}

/// Compute partition ID from VID
///
/// This is the standard partition routing formula used across all services.
/// Partition IDs are 1-indexed: returns values in range [1, partition_num].
///
/// # Example
/// ```
/// use byoridb_common::hash::compute_partition;
///
/// let vid = 12345i64;
/// let partition_num = 10u32;
/// let part_id = compute_partition(vid, partition_num);
/// assert!(part_id >= 1 && part_id <= 10);
/// ```
#[inline]
pub fn compute_partition(vid: i64, partition_num: u32) -> u32 {
    if partition_num == 0 {
        return 1;
    }
    let hash = hash_vid(vid);
    (hash % partition_num as u64) as u32 + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_vid_deterministic() {
        // Same VID always produces same hash
        let h1 = hash_vid(12345);
        let h2 = hash_vid(12345);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_vid_different_inputs() {
        // Different VIDs produce different hashes
        let h1 = hash_vid(1);
        let h2 = hash_vid(2);
        let h3 = hash_vid(3);
        assert_ne!(h1, h2);
        assert_ne!(h2, h3);
        assert_ne!(h1, h3);
    }

    #[test]
    fn test_hash_vid_distribution() {
        // Hashes should be well-distributed across buckets
        let mut buckets = [0u64; 10];
        for vid in 0..10000 {
            let h = hash_vid(vid);
            buckets[(h % 10) as usize] += 1;
        }
        // Each bucket should have ~1000 items (±30%)
        for count in buckets {
            assert!(
                count > 700 && count < 1300,
                "Bucket count {} out of range",
                count
            );
        }
    }

    #[test]
    fn test_hash_bytes_deterministic() {
        let h1 = hash_bytes(b"test_string");
        let h2 = hash_bytes(b"test_string");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_bytes_different_inputs() {
        let h1 = hash_bytes(b"host1:9779");
        let h2 = hash_bytes(b"host2:9779");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_compute_partition_range() {
        // All partitions should be in valid range [1, partition_num]
        for vid in 0..1000 {
            let part = compute_partition(vid, 10);
            assert!(part >= 1 && part <= 10, "Partition {} out of range", part);
        }
    }

    #[test]
    fn test_compute_partition_zero_partitions() {
        // Edge case: 0 partitions should return 1
        assert_eq!(compute_partition(12345, 0), 1);
    }

    #[test]
    fn test_compute_partition_single_partition() {
        // Single partition: all VIDs go to partition 1
        for vid in 0..100 {
            assert_eq!(compute_partition(vid, 1), 1);
        }
    }
}

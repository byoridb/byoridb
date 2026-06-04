// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Hotspot detection for partition management
//!
//! This module provides functionality for:
//! - Tracking partition access patterns (read/write counts)
//! - Detecting hotspot partitions with high QPS
//! - Suggesting partition splits to alleviate hotspots
//!
//! # Example
//!
//! ```ignore
//! use byoridb_meta::hotspot::{HotspotDetector, HotspotConfig};
//! use std::time::Duration;
//!
//! let config = HotspotConfig {
//!     threshold_multiplier: 3.0,
//!     stats_window: Duration::from_secs(60),
//!     auto_split_enabled: false,
//! };
//!
//! let detector = HotspotDetector::new(config);
//!
//! // Record requests
//! detector.record_request(1, 5, false); // space_id=1, part_id=5, read
//! detector.record_request(1, 5, true);  // space_id=1, part_id=5, write
//!
//! // Detect hotspots
//! let hotspots = detector.detect_hotspots(1);
//! for hotspot in hotspots {
//!     println!("Hotspot: partition {} has {:.2}x average QPS", hotspot.part_id, hotspot.ratio);
//! }
//! ```

use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// Configuration for hotspot detection
#[derive(Debug, Clone)]
pub struct HotspotConfig {
    /// Threshold multiplier for hotspot detection (default: 3.0)
    ///
    /// A partition is considered a hotspot if its QPS exceeds
    /// `threshold_multiplier * average_qps`.
    pub threshold_multiplier: f64,

    /// Statistics window duration (default: 60 seconds)
    ///
    /// Request counts are reset periodically based on this window.
    pub stats_window: Duration,

    /// Whether automatic partition splitting is enabled (default: false)
    ///
    /// If enabled, hotspot partitions may be automatically split.
    pub auto_split_enabled: bool,
}

impl Default for HotspotConfig {
    fn default() -> Self {
        Self {
            threshold_multiplier: 3.0,
            stats_window: Duration::from_secs(60),
            auto_split_enabled: false,
        }
    }
}

/// Statistics for a single partition
pub struct PartitionStats {
    /// Number of read operations
    pub read_count: AtomicU64,
    /// Number of write operations
    pub write_count: AtomicU64,
    /// When these stats were last reset
    pub last_reset: Instant,
}

impl PartitionStats {
    fn new() -> Self {
        Self {
            read_count: AtomicU64::new(0),
            write_count: AtomicU64::new(0),
            last_reset: Instant::now(),
        }
    }

    fn total_count(&self) -> u64 {
        self.read_count.load(Ordering::Relaxed) + self.write_count.load(Ordering::Relaxed)
    }

    fn reset(&mut self) {
        self.read_count.store(0, Ordering::Relaxed);
        self.write_count.store(0, Ordering::Relaxed);
        self.last_reset = Instant::now();
    }
}

/// Information about a detected hotspot
#[derive(Debug, Clone)]
pub struct HotspotInfo {
    /// Space ID
    pub space_id: u32,
    /// Partition ID
    pub part_id: u32,
    /// Current QPS for this partition
    pub qps: f64,
    /// Average QPS across all partitions
    pub avg_qps: f64,
    /// Ratio of partition QPS to average QPS
    pub ratio: f64,
    /// Read count
    pub read_count: u64,
    /// Write count
    pub write_count: u64,
}

/// Suggestion for splitting a partition
#[derive(Debug, Clone)]
pub struct SplitSuggestion {
    /// Partition ID to split
    pub part_id: u32,
    /// New partition IDs after split
    pub new_part_ids: Vec<u32>,
    /// Split points for Range strategy (boundary values)
    pub split_points: Vec<i64>,
}

/// Hotspot detector for partition monitoring
pub struct HotspotDetector {
    /// Partition statistics: (space_id, part_id) -> stats
    partition_stats: DashMap<(u32, u32), PartitionStats>,
    /// Detection configuration
    config: HotspotConfig,
}

impl HotspotDetector {
    /// Create a new hotspot detector with the given configuration
    pub fn new(config: HotspotConfig) -> Self {
        Self {
            partition_stats: DashMap::new(),
            config,
        }
    }

    /// Create a new hotspot detector with default configuration
    pub fn with_defaults() -> Self {
        Self::new(HotspotConfig::default())
    }

    /// Record a request to a partition
    ///
    /// # Arguments
    /// * `space_id` - Space ID
    /// * `part_id` - Partition ID
    /// * `is_write` - Whether this is a write operation
    pub fn record_request(&self, space_id: u32, part_id: u32, is_write: bool) {
        let key = (space_id, part_id);

        self.partition_stats
            .entry(key)
            .or_insert_with(PartitionStats::new);

        if let Some(stats) = self.partition_stats.get(&key) {
            if is_write {
                stats.write_count.fetch_add(1, Ordering::Relaxed);
            } else {
                stats.read_count.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Record multiple requests at once
    pub fn record_batch(&self, space_id: u32, part_id: u32, read_count: u64, write_count: u64) {
        let key = (space_id, part_id);

        self.partition_stats
            .entry(key)
            .or_insert_with(PartitionStats::new);

        if let Some(stats) = self.partition_stats.get(&key) {
            stats.read_count.fetch_add(read_count, Ordering::Relaxed);
            stats.write_count.fetch_add(write_count, Ordering::Relaxed);
        }
    }

    /// Detect hotspot partitions for a given space
    ///
    /// A partition is considered a hotspot if its QPS exceeds
    /// `threshold_multiplier * average_qps`.
    pub fn detect_hotspots(&self, space_id: u32) -> Vec<HotspotInfo> {
        let mut hotspots = Vec::new();

        // Collect stats for the space
        let mut partition_counts: Vec<(u32, u64, u64, u64)> = Vec::new(); // (part_id, total, read, write)
        let mut total_requests: u64 = 0;
        let mut partition_count: u64 = 0;

        for entry in self.partition_stats.iter() {
            let ((s_id, part_id), stats) = entry.pair();
            if *s_id != space_id {
                continue;
            }

            let elapsed = stats.last_reset.elapsed();
            if elapsed < Duration::from_micros(1) {
                continue; // Skip if just started
            }

            let read = stats.read_count.load(Ordering::Relaxed);
            let write = stats.write_count.load(Ordering::Relaxed);
            let total = read + write;

            partition_counts.push((*part_id, total, read, write));
            total_requests += total;
            partition_count += 1;
        }

        if partition_count == 0 {
            return hotspots;
        }

        let avg_requests = total_requests as f64 / partition_count as f64;
        let threshold = avg_requests * self.config.threshold_multiplier;

        // Get elapsed time from any partition (they should all have similar reset times)
        let elapsed_secs = if let Some(entry) = self.partition_stats.iter().next() {
            entry.value().last_reset.elapsed().as_secs_f64().max(1.0)
        } else {
            1.0
        };

        let avg_qps = avg_requests / elapsed_secs;

        for (part_id, total, read, write) in partition_counts {
            if (total as f64) > threshold {
                let qps = total as f64 / elapsed_secs;
                let ratio = if avg_requests > 0.0 {
                    total as f64 / avg_requests
                } else {
                    0.0
                };

                hotspots.push(HotspotInfo {
                    space_id,
                    part_id,
                    qps,
                    avg_qps,
                    ratio,
                    read_count: read,
                    write_count: write,
                });

                debug!(
                    "Detected hotspot: space={}, part={}, qps={:.2}, ratio={:.2}x",
                    space_id, part_id, qps, ratio
                );
            }
        }

        if !hotspots.is_empty() {
            info!(
                "Found {} hotspot partitions in space {}",
                hotspots.len(),
                space_id
            );
        }

        hotspots
    }

    /// Suggest a partition split for a hotspot
    ///
    /// For Range strategy, this suggests splitting the partition into two
    /// based on the midpoint of the partition's range.
    ///
    /// For Hash/Modulo strategies, partition splitting requires increasing
    /// the total partition count, which is more complex.
    pub fn suggest_split(
        &self,
        hotspot: &HotspotInfo,
        strategy: &byoridb_common::PartitionStrategy,
        partition_num: u32,
    ) -> Option<SplitSuggestion> {
        match strategy {
            byoridb_common::PartitionStrategy::Range { boundaries } => {
                // For Range strategy, we can split the hotspot partition
                // by adding a new boundary in the middle of its range

                let part_idx = hotspot.part_id as usize - 1;

                // Get the range for this partition
                let lower = if part_idx == 0 {
                    i64::MIN
                } else {
                    boundaries.get(part_idx - 1).copied().unwrap_or(i64::MIN)
                };

                let upper = boundaries.get(part_idx).copied().unwrap_or(i64::MAX);

                // Calculate midpoint (avoiding overflow)
                let midpoint = if lower == i64::MIN && upper == i64::MAX {
                    0
                } else if lower == i64::MIN {
                    upper / 2
                } else if upper == i64::MAX {
                    lower + (i64::MAX - lower) / 2
                } else {
                    lower + (upper - lower) / 2
                };

                info!(
                    "Suggesting split for partition {} at midpoint {}",
                    hotspot.part_id, midpoint
                );

                Some(SplitSuggestion {
                    part_id: hotspot.part_id,
                    new_part_ids: vec![hotspot.part_id, partition_num + 1],
                    split_points: vec![midpoint],
                })
            }
            byoridb_common::PartitionStrategy::Hash | byoridb_common::PartitionStrategy::Modulo => {
                // Hash and Modulo strategies don't support individual partition splits
                // Would need to increase partition_num, which affects all partitions
                warn!(
                    "Partition split not supported for {:?} strategy. Consider increasing partition_num.",
                    strategy
                );
                None
            }
        }
    }

    /// Reset statistics for a space
    pub fn reset_stats(&self, space_id: u32) {
        for mut entry in self.partition_stats.iter_mut() {
            if entry.key().0 == space_id {
                entry.value_mut().reset();
            }
        }
        debug!("Reset hotspot stats for space {}", space_id);
    }

    /// Reset all statistics
    pub fn reset_all_stats(&self) {
        for mut entry in self.partition_stats.iter_mut() {
            entry.value_mut().reset();
        }
        debug!("Reset all hotspot stats");
    }

    /// Get statistics for a specific partition
    pub fn get_partition_stats(&self, space_id: u32, part_id: u32) -> Option<(u64, u64)> {
        self.partition_stats.get(&(space_id, part_id)).map(|stats| {
            (
                stats.read_count.load(Ordering::Relaxed),
                stats.write_count.load(Ordering::Relaxed),
            )
        })
    }

    /// Get total request count for a space
    pub fn get_space_total(&self, space_id: u32) -> u64 {
        let mut total = 0u64;
        for entry in self.partition_stats.iter() {
            if entry.key().0 == space_id {
                total += entry.value().total_count();
            }
        }
        total
    }

    /// Check if auto-split is enabled
    pub fn is_auto_split_enabled(&self) -> bool {
        self.config.auto_split_enabled
    }

    /// Update configuration
    pub fn update_config(&mut self, config: HotspotConfig) {
        self.config = config;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_request() {
        let detector = HotspotDetector::with_defaults();

        detector.record_request(1, 1, false); // read
        detector.record_request(1, 1, true); // write
        detector.record_request(1, 1, false); // read

        let stats = detector.get_partition_stats(1, 1).unwrap();
        assert_eq!(stats.0, 2); // reads
        assert_eq!(stats.1, 1); // writes
    }

    #[test]
    fn test_record_batch() {
        let detector = HotspotDetector::with_defaults();

        detector.record_batch(1, 1, 100, 50);

        let stats = detector.get_partition_stats(1, 1).unwrap();
        assert_eq!(stats.0, 100); // reads
        assert_eq!(stats.1, 50); // writes
    }

    #[test]
    fn test_detect_hotspots() {
        let config = HotspotConfig {
            threshold_multiplier: 2.0,
            stats_window: Duration::from_secs(60),
            auto_split_enabled: false,
        };
        let detector = HotspotDetector::new(config);

        // Create uneven distribution
        detector.record_batch(1, 1, 100, 0); // normal
        detector.record_batch(1, 2, 100, 0); // normal
        detector.record_batch(1, 3, 1000, 0); // hotspot (10x average)

        let hotspots = detector.detect_hotspots(1);
        assert_eq!(hotspots.len(), 1);
        assert_eq!(hotspots[0].part_id, 3);
        assert!(hotspots[0].ratio > 2.0);
    }

    #[test]
    fn test_no_hotspots_even_distribution() {
        let detector = HotspotDetector::with_defaults();

        // Even distribution
        detector.record_batch(1, 1, 100, 0);
        detector.record_batch(1, 2, 100, 0);
        detector.record_batch(1, 3, 100, 0);

        let hotspots = detector.detect_hotspots(1);
        assert!(hotspots.is_empty());
    }

    #[test]
    fn test_reset_stats() {
        let detector = HotspotDetector::with_defaults();

        detector.record_batch(1, 1, 100, 50);
        detector.reset_stats(1);

        let stats = detector.get_partition_stats(1, 1).unwrap();
        assert_eq!(stats.0, 0);
        assert_eq!(stats.1, 0);
    }

    #[test]
    fn test_suggest_split_range() {
        let detector = HotspotDetector::with_defaults();

        let hotspot = HotspotInfo {
            space_id: 1,
            part_id: 2,
            qps: 1000.0,
            avg_qps: 100.0,
            ratio: 10.0,
            read_count: 900,
            write_count: 100,
        };

        let strategy = byoridb_common::PartitionStrategy::Range {
            boundaries: vec![100, 200, 300],
        };

        let suggestion = detector.suggest_split(&hotspot, &strategy, 4);
        assert!(suggestion.is_some());

        let s = suggestion.unwrap();
        assert_eq!(s.part_id, 2);
        assert_eq!(s.new_part_ids.len(), 2);
        assert_eq!(s.split_points.len(), 1);
        // Midpoint between 100 and 200 should be 150
        assert_eq!(s.split_points[0], 150);
    }

    #[test]
    fn test_suggest_split_hash() {
        let detector = HotspotDetector::with_defaults();

        let hotspot = HotspotInfo {
            space_id: 1,
            part_id: 5,
            qps: 1000.0,
            avg_qps: 100.0,
            ratio: 10.0,
            read_count: 900,
            write_count: 100,
        };

        let strategy = byoridb_common::PartitionStrategy::Hash;

        // Hash strategy doesn't support individual partition splits
        let suggestion = detector.suggest_split(&hotspot, &strategy, 10);
        assert!(suggestion.is_none());
    }

    #[test]
    fn test_space_total() {
        let detector = HotspotDetector::with_defaults();

        detector.record_batch(1, 1, 100, 50);
        detector.record_batch(1, 2, 200, 100);
        detector.record_batch(2, 1, 500, 500); // different space

        let total = detector.get_space_total(1);
        assert_eq!(total, 450); // 100+50+200+100
    }
}

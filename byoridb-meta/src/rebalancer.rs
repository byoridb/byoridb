// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Partition rebalancer for distributing partitions across nodes
//!
//! This module provides functionality for:
//! - Planning partition rebalance operations when nodes are added or removed
//! - Executing rebalance plans by generating migration tasks
//! - Checking partition balance across nodes
//!
//! # Example
//!
//! ```ignore
//! use byoridb_meta::rebalancer::PartitionRebalancer;
//!
//! let rebalancer = PartitionRebalancer::new(meta_service.clone());
//!
//! // Plan rebalance for a space
//! let plan = rebalancer.plan_rebalance(space_id).await?;
//!
//! // Execute the rebalance plan
//! let migrations = rebalancer.execute_rebalance(plan).await?;
//! ```

use crate::error::{MetaError, Result};
use crate::service::MetaService;
use crate::storage_client::StorageClient;
use byoridb_common::hash::MigrationTask;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

/// Partition rebalancer for managing partition distribution
pub struct PartitionRebalancer {
    meta_service: Arc<MetaService>,
    storage_client: Arc<StorageClient>,
}

/// Rebalance plan containing migration tasks
#[derive(Debug, Clone)]
pub struct RebalancePlan {
    /// Space ID being rebalanced
    pub space_id: u32,
    /// List of partition migrations to execute
    pub migrations: Vec<MigrationTask>,
    /// Estimated total data size to migrate (in bytes)
    pub estimated_data_size: u64,
}

/// Balance status of partition distribution
#[derive(Debug, Clone, PartialEq)]
pub enum BalanceStatus {
    /// Partitions are evenly distributed
    Balanced,
    /// Partitions are not evenly distributed
    Imbalanced {
        /// Variance in partition counts across nodes
        variance: f64,
        /// Threshold for considering distribution balanced
        threshold: f64,
    },
}

/// Statistics about partition distribution
#[derive(Debug, Clone)]
pub struct PartitionDistributionStats {
    /// Number of partitions per node
    pub partitions_per_node: HashMap<String, u32>,
    /// Mean number of partitions per node
    pub mean: f64,
    /// Variance in partition counts
    pub variance: f64,
    /// Standard deviation
    pub std_dev: f64,
    /// Maximum imbalance ratio (max_count / min_count)
    pub imbalance_ratio: f64,
}

impl PartitionRebalancer {
    /// Create a new partition rebalancer
    pub fn new(meta_service: Arc<MetaService>) -> Self {
        Self {
            meta_service,
            storage_client: Arc::new(StorageClient::new()),
        }
    }

    /// Create a new partition rebalancer with custom storage client
    pub fn with_storage_client(
        meta_service: Arc<MetaService>,
        storage_client: Arc<StorageClient>,
    ) -> Self {
        Self {
            meta_service,
            storage_client,
        }
    }

    /// Plan a rebalance operation for a space
    ///
    /// This method analyzes the current partition distribution and creates
    /// a plan to move partitions to achieve better balance. The plan uses
    /// the consistent hash ring to minimize data movement.
    ///
    /// Note: To compute actual migrations, you need to call `plan_rebalance_with_changes`
    /// which compares before/after states of the hash ring.
    pub async fn plan_rebalance(&self, space_id: u32) -> Result<RebalancePlan> {
        info!("Planning rebalance for space {}", space_id);

        // Get the hash ring for this space to verify it exists
        let _ring_guard = self.meta_service.get_ring(space_id).ok_or_else(|| {
            MetaError::Internal(format!("No hash ring found for space {}", space_id))
        })?;

        // In the simple case, we return an empty plan
        // To get actual migrations, use plan_rebalance_with_changes
        let migrations = Vec::new();

        info!(
            "Rebalance plan for space {}: {} migrations",
            space_id,
            migrations.len()
        );

        Ok(RebalancePlan {
            space_id,
            migrations,
            estimated_data_size: 0,
        })
    }

    /// Plan a rebalance operation by comparing two hash ring states
    ///
    /// This method computes the migrations needed to move from the old
    /// partition assignment to the new one.
    pub fn plan_rebalance_with_changes(
        &self,
        space_id: u32,
        old_ring: &mut byoridb_common::hash::ConsistentHashRing,
        new_ring: &mut byoridb_common::hash::ConsistentHashRing,
    ) -> RebalancePlan {
        let migrations =
            byoridb_common::hash::ConsistentHashRing::compute_migrations(old_ring, new_ring);
        let estimated_data_size = migrations.len() as u64 * 1024 * 1024;

        info!(
            "Rebalance plan for space {}: {} migrations, estimated {} bytes",
            space_id,
            migrations.len(),
            estimated_data_size
        );

        RebalancePlan {
            space_id,
            migrations,
            estimated_data_size,
        }
    }

    /// Execute a rebalance plan
    ///
    /// This method executes the migrations specified in the plan by:
    /// 1. Streaming data from source to target storage nodes
    /// 2. Updating partition ownership on both nodes
    /// 3. Updating metadata to reflect new locations
    pub async fn execute_rebalance(&self, plan: RebalancePlan) -> Result<Vec<MigrationTask>> {
        info!(
            "Executing rebalance for space {}: {} migrations",
            plan.space_id,
            plan.migrations.len()
        );

        if plan.migrations.is_empty() {
            debug!("No migrations needed for space {}", plan.space_id);
            return Ok(vec![]);
        }

        let mut completed_migrations = Vec::new();
        let mut failed_migrations = Vec::new();

        for migration in &plan.migrations {
            info!(
                "Migrating partition {} from {}:{} to {}:{}",
                migration.part_id,
                migration.from.host,
                migration.from.port,
                migration.to.host,
                migration.to.port
            );

            // Execute the actual data migration via storage RPC
            match self
                .storage_client
                .migrate_partition(
                    plan.space_id,
                    migration.part_id,
                    &migration.from.host,
                    migration.from.port,
                    &migration.to.host,
                    migration.to.port,
                )
                .await
            {
                Ok(result) => {
                    info!(
                        "Migration completed: partition {} transferred {} keys",
                        migration.part_id, result.keys_transferred
                    );

                    // Update metadata to reflect new allocation
                    if let Ok(mut hosts) = self
                        .meta_service
                        .get_part_hosts(plan.space_id, migration.part_id)
                        .await
                    {
                        // Remove the old host
                        hosts.retain(|(h, p)| {
                            h != &migration.from.host || *p != migration.from.port
                        });

                        // Add the new host
                        hosts.push((migration.to.host.clone(), migration.to.port));

                        // Update the allocation in meta
                        if let Err(e) = self
                            .meta_service
                            .update_part_allocation(plan.space_id, migration.part_id, hosts)
                            .await
                        {
                            warn!(
                                "Failed to update metadata for partition {}: {}",
                                migration.part_id, e
                            );
                        }
                    }

                    completed_migrations.push(migration.clone());
                }
                Err(e) => {
                    error!("Failed to migrate partition {}: {}", migration.part_id, e);
                    failed_migrations.push((migration.clone(), e.to_string()));
                }
            }
        }

        if !failed_migrations.is_empty() {
            warn!(
                "Rebalance partially completed: {} succeeded, {} failed",
                completed_migrations.len(),
                failed_migrations.len()
            );
        } else {
            info!(
                "Completed rebalance for space {}: {} migrations executed",
                plan.space_id,
                completed_migrations.len()
            );
        }

        Ok(completed_migrations)
    }

    /// Execute rebalance without actual data migration (metadata only)
    ///
    /// This is useful for testing or when data migration is handled externally.
    pub async fn execute_rebalance_metadata_only(
        &self,
        plan: RebalancePlan,
    ) -> Result<Vec<MigrationTask>> {
        info!(
            "Executing metadata-only rebalance for space {}: {} migrations",
            plan.space_id,
            plan.migrations.len()
        );

        if plan.migrations.is_empty() {
            debug!("No migrations needed for space {}", plan.space_id);
            return Ok(vec![]);
        }

        for migration in &plan.migrations {
            debug!(
                "Updating metadata for partition {} from {}:{} to {}:{}",
                migration.part_id,
                migration.from.host,
                migration.from.port,
                migration.to.host,
                migration.to.port
            );

            // Update metadata only
            if let Ok(mut hosts) = self
                .meta_service
                .get_part_hosts(plan.space_id, migration.part_id)
                .await
            {
                hosts.retain(|(h, p)| h != &migration.from.host || *p != migration.from.port);
                hosts.push((migration.to.host.clone(), migration.to.port));

                self.meta_service
                    .update_part_allocation(plan.space_id, migration.part_id, hosts)
                    .await?;
            }
        }

        info!(
            "Completed metadata-only rebalance for space {}: {} migrations",
            plan.space_id,
            plan.migrations.len()
        );

        Ok(plan.migrations)
    }

    /// Check the balance status of partition distribution
    ///
    /// # Arguments
    /// * `allocations` - Map of space_id -> list of (host, partition_count) tuples
    /// * `threshold` - Variance threshold for considering the distribution balanced
    ///
    /// # Returns
    /// `BalanceStatus::Balanced` if variance is below threshold, otherwise `Imbalanced`
    pub fn check_balance(
        &self,
        allocations: &HashMap<u32, Vec<(String, u32)>>,
        threshold: f64,
    ) -> BalanceStatus {
        // Aggregate partition counts per host across all spaces
        let mut host_counts: HashMap<String, u32> = HashMap::new();

        for hosts in allocations.values() {
            for (host, count) in hosts {
                *host_counts.entry(host.clone()).or_insert(0) += count;
            }
        }

        if host_counts.is_empty() {
            return BalanceStatus::Balanced;
        }

        // Calculate variance
        let counts: Vec<f64> = host_counts.values().map(|&c| c as f64).collect();
        let mean: f64 = counts.iter().sum::<f64>() / counts.len() as f64;
        let variance: f64 =
            counts.iter().map(|c| (c - mean).powi(2)).sum::<f64>() / counts.len() as f64;

        debug!(
            "Balance check: {} hosts, mean={:.2}, variance={:.2}, threshold={:.2}",
            host_counts.len(),
            mean,
            variance,
            threshold
        );

        if variance <= threshold {
            BalanceStatus::Balanced
        } else {
            BalanceStatus::Imbalanced {
                variance,
                threshold,
            }
        }
    }

    /// Get detailed partition distribution statistics
    pub fn get_distribution_stats(
        &self,
        allocations: &HashMap<u32, Vec<(String, u32)>>,
    ) -> PartitionDistributionStats {
        let mut host_counts: HashMap<String, u32> = HashMap::new();

        for hosts in allocations.values() {
            for (host, count) in hosts {
                *host_counts.entry(host.clone()).or_insert(0) += count;
            }
        }

        if host_counts.is_empty() {
            return PartitionDistributionStats {
                partitions_per_node: HashMap::new(),
                mean: 0.0,
                variance: 0.0,
                std_dev: 0.0,
                imbalance_ratio: 1.0,
            };
        }

        let counts: Vec<f64> = host_counts.values().map(|&c| c as f64).collect();
        let mean: f64 = counts.iter().sum::<f64>() / counts.len() as f64;
        let variance: f64 =
            counts.iter().map(|c| (c - mean).powi(2)).sum::<f64>() / counts.len() as f64;
        let std_dev = variance.sqrt();

        let min_count = counts.iter().cloned().fold(f64::INFINITY, f64::min);
        let max_count = counts.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let imbalance_ratio = if min_count > 0.0 {
            max_count / min_count
        } else {
            f64::INFINITY
        };

        PartitionDistributionStats {
            partitions_per_node: host_counts,
            mean,
            variance,
            std_dev,
            imbalance_ratio,
        }
    }

    /// Recommend whether rebalancing is needed based on current distribution
    pub async fn should_rebalance(&self, space_id: u32, threshold: f64) -> Result<bool> {
        let allocations = self.meta_service.get_parts_alloc(space_id).await?;

        // Convert to the expected format
        let mut alloc_map: HashMap<String, u32> = HashMap::new();
        for alloc in allocations {
            for (host, port) in alloc.hosts {
                let key = format!("{}:{}", host, port);
                *alloc_map.entry(key).or_insert(0) += 1;
            }
        }

        // Calculate variance
        if alloc_map.is_empty() {
            return Ok(false);
        }

        let counts: Vec<f64> = alloc_map.values().map(|&c| c as f64).collect();
        let mean: f64 = counts.iter().sum::<f64>() / counts.len() as f64;
        let variance: f64 =
            counts.iter().map(|c| (c - mean).powi(2)).sum::<f64>() / counts.len() as f64;

        Ok(variance > threshold)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_balance_status_balanced() {
        use byoridb_kvstore::store::MemoryKVStore;

        let kvstore = Arc::new(MemoryKVStore::new());
        let meta = Arc::new(MetaService::new(kvstore));
        let rebalancer = PartitionRebalancer::new(meta);

        // Even distribution
        let mut allocations = HashMap::new();
        allocations.insert(
            1,
            vec![
                ("host1".to_string(), 10),
                ("host2".to_string(), 10),
                ("host3".to_string(), 10),
            ],
        );

        let status = rebalancer.check_balance(&allocations, 1.0);
        assert_eq!(status, BalanceStatus::Balanced);
    }

    #[test]
    fn test_balance_status_imbalanced() {
        use byoridb_kvstore::store::MemoryKVStore;

        let kvstore = Arc::new(MemoryKVStore::new());
        let meta = Arc::new(MetaService::new(kvstore));
        let rebalancer = PartitionRebalancer::new(meta);

        // Uneven distribution
        let mut allocations = HashMap::new();
        allocations.insert(
            1,
            vec![
                ("host1".to_string(), 100),
                ("host2".to_string(), 10),
                ("host3".to_string(), 10),
            ],
        );

        let status = rebalancer.check_balance(&allocations, 1.0);
        match status {
            BalanceStatus::Imbalanced { variance, .. } => {
                assert!(variance > 1.0);
            }
            _ => panic!("Expected Imbalanced status"),
        }
    }

    #[test]
    fn test_distribution_stats() {
        use byoridb_kvstore::store::MemoryKVStore;

        let kvstore = Arc::new(MemoryKVStore::new());
        let meta = Arc::new(MetaService::new(kvstore));
        let rebalancer = PartitionRebalancer::new(meta);

        let mut allocations = HashMap::new();
        allocations.insert(
            1,
            vec![
                ("host1".to_string(), 10),
                ("host2".to_string(), 20),
                ("host3".to_string(), 30),
            ],
        );

        let stats = rebalancer.get_distribution_stats(&allocations);
        assert_eq!(stats.partitions_per_node.len(), 3);
        assert!((stats.mean - 20.0).abs() < 0.01);
        assert!(stats.imbalance_ratio > 1.0);
    }

    #[test]
    fn test_empty_allocations() {
        use byoridb_kvstore::store::MemoryKVStore;

        let kvstore = Arc::new(MemoryKVStore::new());
        let meta = Arc::new(MetaService::new(kvstore));
        let rebalancer = PartitionRebalancer::new(meta);

        let allocations = HashMap::new();

        let status = rebalancer.check_balance(&allocations, 1.0);
        assert_eq!(status, BalanceStatus::Balanced);

        let stats = rebalancer.get_distribution_stats(&allocations);
        assert_eq!(stats.mean, 0.0);
    }
}

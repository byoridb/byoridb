// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Failure detection and automatic partition reassignment
//!
//! This module provides automatic failure detection and recovery:
//! - Periodically checks for failed storage nodes
//! - Triggers partition reassignment when nodes fail
//! - Notifies storage nodes about leadership changes

use crate::proto::storage::PartitionStatus as ProtoPartitionStatus;
use crate::rebalancer::PartitionRebalancer;
use crate::service::MetaService;
use crate::storage_client::StorageClient;
use byoridb_common::hash::{MigrationTask, RingNode};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Configuration for failure detector
#[derive(Debug, Clone)]
pub struct FailureDetectorConfig {
    /// How often to check for failed nodes
    pub check_interval: Duration,
    /// Time without heartbeat before considering a node dead
    pub failure_timeout: Duration,
    /// Minimum number of healthy nodes required for rebalancing
    pub min_healthy_nodes: usize,
    /// Whether to automatically reassign partitions on failure
    pub auto_reassign: bool,
    /// Maximum concurrent migrations during recovery
    pub max_concurrent_migrations: usize,
}

impl Default for FailureDetectorConfig {
    fn default() -> Self {
        Self {
            check_interval: Duration::from_secs(10),
            failure_timeout: Duration::from_secs(30),
            min_healthy_nodes: 1,
            auto_reassign: true,
            max_concurrent_migrations: 5,
        }
    }
}

/// Failure detector for automatic node failure handling
pub struct FailureDetector {
    meta_service: Arc<MetaService>,
    storage_client: Arc<StorageClient>,
    rebalancer: Arc<PartitionRebalancer>,
    config: FailureDetectorConfig,
    running: Arc<RwLock<bool>>,
}

/// Information about a failed node
#[derive(Debug, Clone)]
pub struct FailedNode {
    pub host: String,
    pub port: u32,
    pub partitions_affected: Vec<(u32, u32)>, // (space_id, part_id)
}

/// Result of failure recovery
#[derive(Debug, Clone)]
pub struct RecoveryResult {
    pub failed_nodes: Vec<FailedNode>,
    pub migrations_executed: Vec<MigrationTask>,
    pub migrations_failed: Vec<(MigrationTask, String)>,
}

impl FailureDetector {
    /// Create a new failure detector
    pub fn new(
        meta_service: Arc<MetaService>,
        storage_client: Arc<StorageClient>,
        config: FailureDetectorConfig,
    ) -> Self {
        let rebalancer = Arc::new(PartitionRebalancer::with_storage_client(
            meta_service.clone(),
            storage_client.clone(),
        ));

        Self {
            meta_service,
            storage_client,
            rebalancer,
            config,
            running: Arc::new(RwLock::new(false)),
        }
    }

    /// Start the failure detector background task
    pub async fn start(&self) -> tokio::task::JoinHandle<()> {
        let meta_service = self.meta_service.clone();
        let storage_client = self.storage_client.clone();
        let config = self.config.clone();
        let running = self.running.clone();
        let rebalancer = self.rebalancer.clone();

        {
            let mut r = running.write().await;
            *r = true;
        }

        tokio::spawn(async move {
            info!(
                "Starting failure detector with check interval {:?}",
                config.check_interval
            );

            let mut interval = tokio::time::interval(config.check_interval);

            loop {
                interval.tick().await;

                // Check if we should stop
                {
                    let r = running.read().await;
                    if !*r {
                        info!("Failure detector stopping");
                        break;
                    }
                }

                // Run failure detection
                if let Err(e) =
                    Self::check_and_recover(&meta_service, &storage_client, &rebalancer, &config)
                        .await
                {
                    error!("Failure detection error: {}", e);
                }
            }
        })
    }

    /// Stop the failure detector
    pub async fn stop(&self) {
        let mut running = self.running.write().await;
        *running = false;
        info!("Failure detector stopped");
    }

    /// Check if running
    pub async fn is_running(&self) -> bool {
        *self.running.read().await
    }

    /// Run a single check and recovery cycle
    async fn check_and_recover(
        meta_service: &Arc<MetaService>,
        storage_client: &Arc<StorageClient>,
        _rebalancer: &Arc<PartitionRebalancer>,
        config: &FailureDetectorConfig,
    ) -> Result<Option<RecoveryResult>, Box<dyn std::error::Error + Send + Sync>> {
        // 1. Clean up stale hosts and get the list of removed hosts
        let removed_hosts = meta_service.cleanup_stale_hosts();

        if removed_hosts.is_empty() {
            debug!("No stale hosts detected");
            return Ok(None);
        }

        info!(
            "Detected {} failed hosts: {:?}",
            removed_hosts.len(),
            removed_hosts
        );

        // 2. Get remaining healthy hosts
        let healthy_hosts = meta_service.get_active_storage_hosts();

        if healthy_hosts.len() < config.min_healthy_nodes {
            warn!(
                "Not enough healthy nodes for recovery: {} < {}",
                healthy_hosts.len(),
                config.min_healthy_nodes
            );
            return Ok(None);
        }

        if !config.auto_reassign {
            info!("Auto reassignment disabled, skipping partition migration");
            return Ok(None);
        }

        // 3. Find partitions affected by the failed hosts
        let mut failed_nodes = Vec::new();
        let mut all_affected_partitions: Vec<(u32, u32)> = Vec::new();

        for (host, port) in &removed_hosts {
            let partitions = meta_service.get_host_partitions(host, *port);
            if !partitions.is_empty() {
                all_affected_partitions.extend(partitions.iter().cloned());
                failed_nodes.push(FailedNode {
                    host: host.clone(),
                    port: *port,
                    partitions_affected: partitions,
                });
            }
        }

        if all_affected_partitions.is_empty() {
            info!("No partitions affected by failed hosts");
            return Ok(Some(RecoveryResult {
                failed_nodes,
                migrations_executed: vec![],
                migrations_failed: vec![],
            }));
        }

        info!(
            "Found {} partitions affected by node failures",
            all_affected_partitions.len()
        );

        // 4. For each affected partition, reassign to a healthy host
        let mut migrations_executed = Vec::new();
        let mut migrations_failed = Vec::new();

        // Group by space_id for efficient processing
        let mut space_partitions: std::collections::HashMap<u32, Vec<u32>> =
            std::collections::HashMap::new();
        for (space_id, part_id) in all_affected_partitions {
            space_partitions.entry(space_id).or_default().push(part_id);
        }

        for (space_id, part_ids) in space_partitions {
            // Get partition allocation info
            for part_id in part_ids {
                // Find a healthy host to reassign to
                if let Some((target_host, target_port)) = healthy_hosts.first() {
                    // Find the failed host for this partition
                    let failed_host = failed_nodes
                        .iter()
                        .find(|n| n.partitions_affected.contains(&(space_id, part_id)));

                    if let Some(failed) = failed_host {
                        // Create migration task
                        let migration = MigrationTask {
                            part_id,
                            from: RingNode::new(failed.host.clone(), failed.port),
                            to: RingNode::new(target_host.clone(), *target_port),
                            is_primary: true,
                        };

                        // Since the source node is dead, we can only update metadata
                        // and notify the target to become the new owner
                        info!(
                            "Reassigning partition {}:{} from dead node {}:{} to {}:{}",
                            space_id, part_id, failed.host, failed.port, target_host, target_port
                        );

                        // Update metadata
                        if let Ok(mut hosts) = meta_service.get_part_hosts(space_id, part_id).await
                        {
                            hosts.retain(|(h, p)| h != &failed.host || *p != failed.port);
                            hosts.push((target_host.clone(), *target_port));

                            if let Err(e) = meta_service
                                .update_part_allocation(space_id, part_id, hosts)
                                .await
                            {
                                warn!(
                                    "Failed to update metadata for partition {}:{}: {}",
                                    space_id, part_id, e
                                );
                                migrations_failed.push((migration, e.to_string()));
                                continue;
                            }
                        }

                        // Notify the target node about its new partition ownership
                        if let Err(e) = storage_client
                            .notify_ownership_change(
                                target_host,
                                *target_port,
                                space_id,
                                part_id,
                                ProtoPartitionStatus::PsLeader,
                                Some((target_host.clone(), *target_port)),
                                vec![(target_host.clone(), *target_port)],
                            )
                            .await
                        {
                            warn!(
                                "Failed to notify target node about partition {}:{}: {}",
                                space_id, part_id, e
                            );
                            // Don't fail the migration, metadata is already updated
                        }

                        migrations_executed.push(migration);
                    }
                }
            }
        }

        info!(
            "Recovery completed: {} migrations executed, {} failed",
            migrations_executed.len(),
            migrations_failed.len()
        );

        Ok(Some(RecoveryResult {
            failed_nodes,
            migrations_executed,
            migrations_failed,
        }))
    }

    /// Manually trigger failure check (for testing or manual intervention)
    pub async fn trigger_check(
        &self,
    ) -> Result<Option<RecoveryResult>, Box<dyn std::error::Error + Send + Sync>> {
        Self::check_and_recover(
            &self.meta_service,
            &self.storage_client,
            &self.rebalancer,
            &self.config,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use byoridb_kvstore::store::MemoryKVStore;

    #[test]
    fn test_failure_detector_config_default() {
        let config = FailureDetectorConfig::default();
        assert_eq!(config.check_interval, Duration::from_secs(10));
        assert_eq!(config.failure_timeout, Duration::from_secs(30));
        assert!(config.auto_reassign);
    }

    #[tokio::test]
    async fn test_failure_detector_creation() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let meta_service = Arc::new(MetaService::new(kvstore));
        let storage_client = Arc::new(StorageClient::new());
        let config = FailureDetectorConfig::default();

        let detector = FailureDetector::new(meta_service, storage_client, config);
        assert!(!detector.is_running().await);
    }
}

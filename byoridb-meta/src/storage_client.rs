// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Storage RPC client for data migration operations
//!
//! This module provides a client for communicating with storage nodes
//! to execute data migrations during partition rebalancing.

use crate::error::{MetaError, Result};
use crate::proto::storage::{
    storage_service_client::StorageServiceClient, ErrorCode, GetPartitionDataRequest,
    GetPartitionStatusRequest, PartitionDataChunk, PartitionStatus as ProtoPartitionStatus,
    UpdatePartitionOwnershipRequest,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_stream::StreamExt;
use tonic::transport::Channel;
use tracing::{debug, error, info, warn};

/// Storage client for RPC communication with storage nodes
pub struct StorageClient {
    /// Cached connections to storage nodes: (host, port) -> client
    #[allow(clippy::type_complexity)]
    connections: Arc<RwLock<HashMap<(String, u32), StorageServiceClient<Channel>>>>,
}

impl StorageClient {
    /// Create a new storage client
    pub fn new() -> Self {
        Self {
            connections: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get or create a connection to a storage node
    async fn get_connection(&self, host: &str, port: u32) -> Result<StorageServiceClient<Channel>> {
        let key = (host.to_string(), port);

        // Check if we already have a connection
        {
            let connections = self.connections.read().await;
            if let Some(client) = connections.get(&key) {
                return Ok(client.clone());
            }
        }

        // Create a new connection
        let addr = format!("http://{}:{}", host, port);
        debug!("Creating new connection to storage node at {}", addr);

        let channel = Channel::from_shared(addr.clone())
            .map_err(|e| MetaError::Internal(format!("Invalid address {}: {}", addr, e)))?
            .connect()
            .await
            .map_err(|e| MetaError::Internal(format!("Failed to connect to {}: {}", addr, e)))?;

        let client = StorageServiceClient::new(channel);

        // Cache the connection
        {
            let mut connections = self.connections.write().await;
            connections.insert(key, client.clone());
        }

        Ok(client)
    }

    /// Close a connection to a storage node
    pub async fn close_connection(&self, host: &str, port: u32) {
        let key = (host.to_string(), port);
        let mut connections = self.connections.write().await;
        connections.remove(&key);
        debug!("Closed connection to storage node {}:{}", host, port);
    }

    /// Clear all cached connections
    pub async fn clear_connections(&self) {
        let mut connections = self.connections.write().await;
        connections.clear();
        debug!("Cleared all storage node connections");
    }

    /// Execute a partition migration from source to target node
    ///
    /// This method:
    /// 1. Streams partition data from the source node
    /// 2. Sends the data to the target node
    /// 3. Updates ownership on both nodes
    pub async fn migrate_partition(
        &self,
        space_id: u32,
        part_id: u32,
        source_host: &str,
        source_port: u32,
        target_host: &str,
        target_port: u32,
    ) -> Result<MigrationResult> {
        info!(
            "Starting partition migration: space={}, part={}, from {}:{} to {}:{}",
            space_id, part_id, source_host, source_port, target_host, target_port
        );

        // 1. Get source client
        let mut source_client = self.get_connection(source_host, source_port).await?;

        // 2. Get target client
        let mut target_client = self.get_connection(target_host, target_port).await?;

        // 3. Mark source partition as transferring
        self.update_partition_ownership(
            &mut source_client,
            space_id,
            part_id,
            ProtoPartitionStatus::PsTransferring,
            None,
        )
        .await?;

        // 4. Stream data from source
        let request = GetPartitionDataRequest {
            space_id,
            part_id,
            key_prefix: Vec::new(),
        };

        let response = source_client
            .get_partition_data(request)
            .await
            .map_err(|e| MetaError::Internal(format!("Failed to get partition data: {}", e)))?;

        let mut data_stream = response.into_inner();

        // 5. Create channel for sending to target
        let (tx, rx) = tokio::sync::mpsc::channel::<PartitionDataChunk>(32);

        // 6. Spawn task to forward data from source to target channel
        let forward_task = tokio::spawn(async move {
            let mut total_keys = 0u64;
            while let Some(chunk_result) = data_stream.next().await {
                match chunk_result {
                    Ok(chunk) => {
                        total_keys += chunk.data.len() as u64;
                        let done = chunk.done;
                        if tx.send(chunk).await.is_err() {
                            warn!("Failed to forward chunk to target");
                            break;
                        }
                        if done {
                            break;
                        }
                    }
                    Err(e) => {
                        error!("Error receiving data from source: {}", e);
                        break;
                    }
                }
            }
            total_keys
        });

        // 7. Send data stream to target
        let receiver_stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        let transfer_response = target_client
            .transfer_partition(receiver_stream)
            .await
            .map_err(|e| MetaError::Internal(format!("Failed to transfer partition: {}", e)))?
            .into_inner();

        // Wait for forward task to complete
        let total_keys = forward_task
            .await
            .map_err(|e| MetaError::Internal(format!("Forward task failed: {}", e)))?;

        // 8. Check transfer result
        if transfer_response.code != ErrorCode::Succeeded as i32 {
            error!(
                "Partition transfer failed: {}",
                transfer_response.error_message
            );
            // Rollback: mark source as leader again
            self.update_partition_ownership(
                &mut source_client,
                space_id,
                part_id,
                ProtoPartitionStatus::PsLeader,
                None,
            )
            .await?;

            return Err(MetaError::Internal(format!(
                "Transfer failed: {}",
                transfer_response.error_message
            )));
        }

        // 9. Update ownership: mark target as leader, source as not owned
        self.update_partition_ownership(
            &mut target_client,
            space_id,
            part_id,
            ProtoPartitionStatus::PsLeader,
            Some((target_host.to_string(), target_port)),
        )
        .await?;

        self.update_partition_ownership(
            &mut source_client,
            space_id,
            part_id,
            ProtoPartitionStatus::PsNotOwned,
            None,
        )
        .await?;

        info!(
            "Partition migration completed: space={}, part={}, keys={}",
            space_id, part_id, total_keys
        );

        Ok(MigrationResult {
            space_id,
            part_id,
            keys_transferred: transfer_response.keys_received,
            source: (source_host.to_string(), source_port),
            target: (target_host.to_string(), target_port),
        })
    }

    /// Update partition ownership on a storage node
    async fn update_partition_ownership(
        &self,
        client: &mut StorageServiceClient<Channel>,
        space_id: u32,
        part_id: u32,
        status: ProtoPartitionStatus,
        leader: Option<(String, u32)>,
    ) -> Result<()> {
        let leader_addr = leader.map(|(host, port)| crate::proto::storage::HostAddr { host, port });

        let request = UpdatePartitionOwnershipRequest {
            space_id,
            part_id,
            new_status: status as i32,
            leader: leader_addr,
            replicas: vec![],
        };

        let response = client
            .update_partition_ownership(request)
            .await
            .map_err(|e| {
                MetaError::Internal(format!("Failed to update partition ownership: {}", e))
            })?
            .into_inner();

        if response.code != ErrorCode::Succeeded as i32 {
            return Err(MetaError::Internal(format!(
                "Update ownership failed: {}",
                response.error_message
            )));
        }

        Ok(())
    }

    /// Get partition status from a storage node
    pub async fn get_partition_status(
        &self,
        host: &str,
        port: u32,
        space_id: u32,
        part_id: u32,
    ) -> Result<PartitionStatus> {
        let mut client = self.get_connection(host, port).await?;

        let request = GetPartitionStatusRequest { space_id, part_id };

        let response = client
            .get_partition_status(request)
            .await
            .map_err(|e| MetaError::Internal(format!("Failed to get partition status: {}", e)))?
            .into_inner();

        if response.code != ErrorCode::Succeeded as i32 {
            return Ok(PartitionStatus {
                status: ProtoPartitionStatus::PsNotOwned,
                leader: None,
                replicas: vec![],
                key_count: 0,
                data_size_bytes: 0,
            });
        }

        let leader = response.leader.map(|addr| (addr.host.clone(), addr.port));

        let replicas = response
            .replicas
            .iter()
            .map(|addr| (addr.host.clone(), addr.port))
            .collect();

        Ok(PartitionStatus {
            status: ProtoPartitionStatus::try_from(response.status)
                .unwrap_or(ProtoPartitionStatus::PsNotOwned),
            leader,
            replicas,
            key_count: response.key_count,
            data_size_bytes: response.data_size_bytes,
        })
    }

    /// Notify a storage node about new partition ownership
    #[allow(clippy::too_many_arguments)]
    pub async fn notify_ownership_change(
        &self,
        host: &str,
        port: u32,
        space_id: u32,
        part_id: u32,
        status: ProtoPartitionStatus,
        leader: Option<(String, u32)>,
        replicas: Vec<(String, u32)>,
    ) -> Result<()> {
        let mut client = self.get_connection(host, port).await?;

        let leader_addr = leader.map(|(h, p)| crate::proto::storage::HostAddr { host: h, port: p });
        let replica_addrs = replicas
            .into_iter()
            .map(|(h, p)| crate::proto::storage::HostAddr { host: h, port: p })
            .collect();

        let request = UpdatePartitionOwnershipRequest {
            space_id,
            part_id,
            new_status: status as i32,
            leader: leader_addr,
            replicas: replica_addrs,
        };

        let response = client
            .update_partition_ownership(request)
            .await
            .map_err(|e| MetaError::Internal(format!("Failed to notify ownership change: {}", e)))?
            .into_inner();

        if response.code != ErrorCode::Succeeded as i32 {
            warn!(
                "Failed to notify storage node {}:{} about ownership change: {}",
                host, port, response.error_message
            );
        }

        Ok(())
    }
}

impl Default for StorageClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of a partition migration
#[derive(Debug, Clone)]
pub struct MigrationResult {
    pub space_id: u32,
    pub part_id: u32,
    pub keys_transferred: u64,
    pub source: (String, u32),
    pub target: (String, u32),
}

/// Partition status information
#[derive(Debug, Clone)]
pub struct PartitionStatus {
    pub status: ProtoPartitionStatus,
    pub leader: Option<(String, u32)>,
    pub replicas: Vec<(String, u32)>,
    pub key_count: u64,
    pub data_size_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_client_creation() {
        let client = StorageClient::new();
        assert!(client.connections.try_read().is_ok());
    }
}

// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Storage Query Client for distributed query execution
//!
//! This module provides a client for executing queries against remote Storage nodes.
//! It handles connection pooling, retries, and error handling.

use byoridb_storage::proto::storage::{
    storage_service_client::StorageServiceClient, BatchGetEdgesRequest, BatchGetEdgesResponse,
    BatchGetVerticesRequest, BatchGetVerticesResponse, BloomFilterType, CheckBloomFilterRequest,
    CheckBloomFilterResponse, EdgeKey, GetNeighborsBySourceRequest, GetNeighborsBySourceResponse,
    IndexValue, LookupEdgeIndexRequest, LookupEdgeIndexResponse, LookupTagIndexRequest,
    LookupTagIndexResponse, ScanEdgesRequest, ScanEdgesResponse, ScanVerticesRequest,
    ScanVerticesResponse,
};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tonic::transport::Channel;
use tracing::debug;

/// Error type for storage client operations
#[derive(Error, Debug)]
pub enum StorageClientError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("RPC error: {0}")]
    RpcError(#[from] tonic::Status),

    #[error("Transport error: {0}")]
    TransportError(#[from] tonic::transport::Error),

    #[error("Storage error: code={code}, message={message}")]
    StorageError { code: i32, message: String },

    #[error("Invalid response: {0}")]
    InvalidResponse(String),
}

pub type Result<T> = std::result::Result<T, StorageClientError>;

/// Configuration for storage query client
#[derive(Debug, Clone)]
pub struct StorageQueryClientConfig {
    /// Connection timeout
    pub connect_timeout: Duration,
    /// Request timeout
    pub request_timeout: Duration,
    /// Maximum connections per host
    pub max_connections_per_host: usize,
    /// Enable connection pooling
    pub enable_pooling: bool,
}

impl Default for StorageQueryClientConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(30),
            max_connections_per_host: 10,
            enable_pooling: true,
        }
    }
}

/// Storage Query Client for distributed query execution
///
/// Provides methods to execute queries against remote Storage nodes:
/// - batch_get_vertices: Fetch vertices by VID list
/// - batch_get_edges: Fetch edges by EdgeKey list
/// - scan_vertices: Scan vertices in a partition
/// - scan_edges: Scan edges in a partition
pub struct StorageQueryClient {
    /// Cached connections: (host:port) -> Channel
    connections: Arc<RwLock<HashMap<String, Channel>>>,
    /// Client configuration
    config: StorageQueryClientConfig,
}

impl StorageQueryClient {
    /// Create a new storage query client
    pub fn new() -> Self {
        Self::with_config(StorageQueryClientConfig::default())
    }

    /// Create a new storage query client with custom config
    pub fn with_config(config: StorageQueryClientConfig) -> Self {
        Self {
            connections: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// Get or create a connection to a storage host
    async fn get_connection(&self, host: &str, port: u32) -> Result<Channel> {
        let addr = format!("{}:{}", host, port);

        // Check cache first
        {
            let cache = self.connections.read();
            if let Some(channel) = cache.get(&addr) {
                return Ok(channel.clone());
            }
        }

        // Create new connection
        debug!("Creating new connection to storage node: {}", addr);

        let endpoint = format!("http://{}", addr);
        let channel = Channel::from_shared(endpoint)
            .map_err(|e| StorageClientError::ConnectionFailed(e.to_string()))?
            .connect_timeout(self.config.connect_timeout)
            .timeout(self.config.request_timeout)
            .connect()
            .await?;

        // Cache the connection
        if self.config.enable_pooling {
            let mut cache = self.connections.write();
            cache.insert(addr.clone(), channel.clone());
        }

        Ok(channel)
    }

    /// Create a gRPC client for a storage host
    async fn create_client(&self, host: &str, port: u32) -> Result<StorageServiceClient<Channel>> {
        let channel = self.get_connection(host, port).await?;
        Ok(StorageServiceClient::new(channel))
    }

    /// Batch get vertices from a specific storage host
    pub async fn batch_get_vertices(
        &self,
        host: &str,
        port: u32,
        space_id: u32,
        vids: Vec<i64>,
        tag_names: Vec<String>,
        prop_names: Vec<String>,
    ) -> Result<BatchGetVerticesResponse> {
        debug!(
            "BatchGetVertices: host={}:{}, space_id={}, vids_count={}",
            host,
            port,
            space_id,
            vids.len()
        );

        let mut client = self.create_client(host, port).await?;

        let request = BatchGetVerticesRequest {
            space_id,
            vids,
            tag_names,
            prop_names,
        };

        let response = client.batch_get_vertices(request).await?;
        let inner = response.into_inner();

        // Check for storage-level errors
        if inner.code != 0 {
            return Err(StorageClientError::StorageError {
                code: inner.code,
                message: inner.error_message.clone(),
            });
        }

        Ok(inner)
    }

    /// Batch get edges from a specific storage host
    pub async fn batch_get_edges(
        &self,
        host: &str,
        port: u32,
        space_id: u32,
        edge_keys: Vec<EdgeKey>,
        prop_names: Vec<String>,
    ) -> Result<BatchGetEdgesResponse> {
        debug!(
            "BatchGetEdges: host={}:{}, space_id={}, edge_keys_count={}",
            host,
            port,
            space_id,
            edge_keys.len()
        );

        let mut client = self.create_client(host, port).await?;

        let request = BatchGetEdgesRequest {
            space_id,
            edge_keys,
            prop_names,
        };

        let response = client.batch_get_edges(request).await?;
        let inner = response.into_inner();

        if inner.code != 0 {
            return Err(StorageClientError::StorageError {
                code: inner.code,
                message: inner.error_message.clone(),
            });
        }

        Ok(inner)
    }

    /// Scan vertices in a specific partition
    #[allow(clippy::too_many_arguments)]
    pub async fn scan_vertices(
        &self,
        host: &str,
        port: u32,
        space_id: u32,
        part_id: u32,
        tag_name: String,
        prop_names: Vec<String>,
        cursor: Vec<u8>,
        limit: u32,
    ) -> Result<ScanVerticesResponse> {
        debug!(
            "ScanVertices: host={}:{}, space_id={}, part_id={}, tag={}",
            host, port, space_id, part_id, tag_name
        );

        let mut client = self.create_client(host, port).await?;

        let request = ScanVerticesRequest {
            space_id,
            part_id,
            tag_name,
            prop_names,
            cursor,
            limit,
        };

        let response = client.scan_vertices(request).await?;
        let inner = response.into_inner();

        if inner.code != 0 {
            return Err(StorageClientError::StorageError {
                code: inner.code,
                message: inner.error_message.clone(),
            });
        }

        Ok(inner)
    }

    /// Scan edges in a specific partition
    #[allow(clippy::too_many_arguments)]
    pub async fn scan_edges(
        &self,
        host: &str,
        port: u32,
        space_id: u32,
        part_id: u32,
        edge_type: String,
        prop_names: Vec<String>,
        cursor: Vec<u8>,
        limit: u32,
    ) -> Result<ScanEdgesResponse> {
        debug!(
            "ScanEdges: host={}:{}, space_id={}, part_id={}, edge_type={}",
            host, port, space_id, part_id, edge_type
        );

        let mut client = self.create_client(host, port).await?;

        let request = ScanEdgesRequest {
            space_id,
            part_id,
            edge_type,
            prop_names,
            cursor,
            limit,
        };

        let response = client.scan_edges(request).await?;
        let inner = response.into_inner();

        if inner.code != 0 {
            return Err(StorageClientError::StorageError {
                code: inner.code,
                message: inner.error_message.clone(),
            });
        }

        Ok(inner)
    }

    /// Targeted neighbor fetch by source-VID list.
    ///
    /// Prefer this over [`scan_edges`] when the query knows the source VIDs
    /// up front (the distributed `GO` case). Each source is read with an
    /// O(degree) prefix scan on the server, so a partition with 10M edges
    /// and a query touching 5 source vertices does ~5·d work instead of
    /// scanning all 10M.
    #[allow(clippy::too_many_arguments)]
    pub async fn get_neighbors_by_source(
        &self,
        host: &str,
        port: u32,
        space_id: u32,
        part_id: u32,
        src_vids: Vec<i64>,
        edge_types: Vec<String>,
        limit_per_src: u32,
        prop_names: Vec<String>,
    ) -> Result<GetNeighborsBySourceResponse> {
        debug!(
            "GetNeighborsBySource: host={}:{}, space_id={}, part_id={}, src_count={}, edge_types={:?}",
            host,
            port,
            space_id,
            part_id,
            src_vids.len(),
            edge_types
        );

        let mut client = self.create_client(host, port).await?;

        let request = GetNeighborsBySourceRequest {
            space_id,
            part_id,
            src_vids,
            edge_types,
            limit_per_src,
            prop_names,
        };

        let response = client.get_neighbors_by_source(request).await?;
        let inner = response.into_inner();

        if inner.code != 0 {
            return Err(StorageClientError::StorageError {
                code: inner.code,
                message: inner.error_message.clone(),
            });
        }

        Ok(inner)
    }

    // ===== Index Lookup Methods =====

    /// Lookup vertices by tag index in a specific partition
    #[allow(clippy::too_many_arguments)]
    pub async fn lookup_tag_index(
        &self,
        host: &str,
        port: u32,
        space_id: u32,
        part_id: u32,
        index_id: u32,
        index_name: String,
        values: Vec<IndexValue>,
        limit: u32,
        cursor: Vec<u8>,
    ) -> Result<LookupTagIndexResponse> {
        debug!(
            "LookupTagIndex: host={}:{}, space_id={}, part_id={}, index_id={}, index_name={}",
            host, port, space_id, part_id, index_id, index_name
        );

        let mut client = self.create_client(host, port).await?;

        let request = LookupTagIndexRequest {
            space_id,
            part_id,
            index_id,
            index_name,
            values,
            limit,
            cursor,
        };

        let response = client.lookup_tag_index(request).await?;
        let inner = response.into_inner();

        if inner.code != 0 {
            return Err(StorageClientError::StorageError {
                code: inner.code,
                message: inner.error_message.clone(),
            });
        }

        Ok(inner)
    }

    /// Lookup edges by edge index in a specific partition
    #[allow(clippy::too_many_arguments)]
    pub async fn lookup_edge_index(
        &self,
        host: &str,
        port: u32,
        space_id: u32,
        part_id: u32,
        index_id: u32,
        index_name: String,
        values: Vec<IndexValue>,
        limit: u32,
        cursor: Vec<u8>,
    ) -> Result<LookupEdgeIndexResponse> {
        debug!(
            "LookupEdgeIndex: host={}:{}, space_id={}, part_id={}, index_id={}, index_name={}",
            host, port, space_id, part_id, index_id, index_name
        );

        let mut client = self.create_client(host, port).await?;

        let request = LookupEdgeIndexRequest {
            space_id,
            part_id,
            index_id,
            index_name,
            values,
            limit,
            cursor,
        };

        let response = client.lookup_edge_index(request).await?;
        let inner = response.into_inner();

        if inner.code != 0 {
            return Err(StorageClientError::StorageError {
                code: inner.code,
                message: inner.error_message.clone(),
            });
        }

        Ok(inner)
    }

    /// Check if keys exist using Bloom filter (fast existence check)
    pub async fn check_bloom_filter(
        &self,
        host: &str,
        port: u32,
        space_id: u32,
        part_id: u32,
        filter_type: BloomFilterType,
        keys: Vec<i64>,
    ) -> Result<CheckBloomFilterResponse> {
        debug!(
            "CheckBloomFilter: host={}:{}, space_id={}, part_id={}, keys_count={}",
            host,
            port,
            space_id,
            part_id,
            keys.len()
        );

        let mut client = self.create_client(host, port).await?;

        let request = CheckBloomFilterRequest {
            space_id,
            part_id,
            filter_type: filter_type as i32,
            keys,
        };

        let response = client.check_bloom_filter(request).await?;
        let inner = response.into_inner();

        if inner.code != 0 {
            return Err(StorageClientError::StorageError {
                code: inner.code,
                message: inner.error_message.clone(),
            });
        }

        Ok(inner)
    }

    /// Clear all cached connections
    pub fn clear_connections(&self) {
        let mut cache = self.connections.write();
        cache.clear();
        debug!("Cleared all cached connections");
    }

    /// Remove a specific connection from cache
    pub fn remove_connection(&self, host: &str, port: u32) {
        let addr = format!("{}:{}", host, port);
        let mut cache = self.connections.write();
        if cache.remove(&addr).is_some() {
            debug!("Removed connection from cache: {}", addr);
        }
    }

    /// Get the number of cached connections
    pub fn connection_count(&self) -> usize {
        let cache = self.connections.read();
        cache.len()
    }
}

impl Default for StorageQueryClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = StorageQueryClientConfig::default();
        assert_eq!(config.connect_timeout, Duration::from_secs(5));
        assert_eq!(config.request_timeout, Duration::from_secs(30));
        assert!(config.enable_pooling);
    }

    #[test]
    fn test_client_creation() {
        let client = StorageQueryClient::new();
        assert_eq!(client.connection_count(), 0);
    }

    #[test]
    fn test_clear_connections() {
        let client = StorageQueryClient::new();
        // Manually add a fake connection for testing
        {
            let mut cache = client.connections.write();
            // We can't add a real channel without connecting, but we can verify the clear works
            assert!(cache.is_empty());
        }
        client.clear_connections();
        assert_eq!(client.connection_count(), 0);
    }
}

// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Meta client for remote Meta service communication
//!
//! This module provides a gRPC client for communicating with the Meta service.
//! It includes:
//! - Connection management with automatic reconnection
//! - Retry logic with exponential backoff
//! - Local caching for schema metadata
//! - Leader tracking for distributed deployments
//!
//! # Usage
//!
//! ```ignore
//! use byoridb_meta::MetaClient;
//!
//! let client = MetaClient::new("localhost:9559").await?;
//!
//! // Get space schema (cached)
//! let space = client.get_space("my_graph").await?;
//!
//! // Get tag schema (cached)
//! let tag = client.get_tag(space.id, "person").await?;
//! ```

use crate::error::{MetaError, Result};
use crate::proto::meta_service_client::MetaServiceClient;
use crate::proto::*;
use crate::schema::{
    DataType as SchemaDataType, EdgeIndex, EdgeSchema, Field as SchemaField, PartAllocation, Space,
    TagIndex, TagSchema, VidType as SchemaVidType,
};
use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tonic::transport::{Channel, Endpoint};
use tracing::{debug, info, warn};

/// Configuration for the Meta client
#[derive(Debug, Clone)]
pub struct MetaClientConfig {
    /// Initial addresses to connect to
    pub addrs: Vec<String>,
    /// Connection timeout
    pub connect_timeout: Duration,
    /// Request timeout
    pub request_timeout: Duration,
    /// Maximum retry attempts
    pub max_retries: u32,
    /// Base delay for exponential backoff
    pub retry_base_delay: Duration,
    /// Cache TTL for schema metadata
    ///
    /// # Cache Consistency Warning
    /// Schema metadata is cached locally. In a distributed environment, schema changes
    /// (ALTER, DROP) may not be immediately visible to other clients until the TTL expires.
    ///
    /// Recommended settings:
    /// - Development: Low TTL (e.g. 1-5 seconds)
    /// - Production: Higher TTL (e.g. 300 seconds) for performance, unless
    ///   schema changes are frequent.
    pub cache_ttl: Duration,
}

impl Default for MetaClientConfig {
    fn default() -> Self {
        Self {
            addrs: vec!["localhost:9559".to_string()],
            connect_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(10),
            max_retries: 3,
            retry_base_delay: Duration::from_millis(100),
            cache_ttl: Duration::from_secs(300), // 5 minutes
        }
    }
}

/// Cached item with expiration
struct CacheEntry<T> {
    value: T,
    expires_at: Instant,
}

impl<T: Clone> CacheEntry<T> {
    fn new(value: T, ttl: Duration) -> Self {
        Self {
            value,
            expires_at: Instant::now() + ttl,
        }
    }

    fn is_expired(&self) -> bool {
        Instant::now() > self.expires_at
    }

    fn get(&self) -> Option<T> {
        if self.is_expired() {
            None
        } else {
            Some(self.value.clone())
        }
    }
}

/// Liveness state reported for a storage host.
///
/// Meta continues to list a host after its heartbeat lapses so operators can
/// see nodes that recently dropped out; those entries carry
/// [`HostLiveness::Offline`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostLiveness {
    Online,
    Offline,
}

/// Snapshot of a storage host returned by [`MetaClient::list_hosts`].
#[derive(Debug, Clone)]
pub struct HostInfo {
    pub host: String,
    pub port: u32,
    pub status: HostLiveness,
    /// Number of partitions for which this host is the designated leader
    /// (first host in the allocation's host list).
    pub leader_count: u64,
    /// Total number of partitions assigned to this host across all spaces.
    pub part_count: u64,
}

/// Meta client with caching and retry support
pub struct MetaClient {
    /// gRPC client connection
    client: RwLock<Option<MetaServiceClient<Channel>>>,
    /// Configuration
    config: MetaClientConfig,
    /// Current leader address
    leader: RwLock<Option<HostAddr>>,
    /// Cache: space_name -> Space
    space_cache: DashMap<String, CacheEntry<Space>>,
    /// Cache: (space_id, tag_name) -> TagSchema
    tag_cache: DashMap<(u32, String), CacheEntry<TagSchema>>,
    /// Cache: (space_id, edge_name) -> EdgeSchema
    edge_cache: DashMap<(u32, String), CacheEntry<EdgeSchema>>,
    /// Cache: space_id -> Vec<PartAllocation>
    parts_cache: DashMap<u32, CacheEntry<Vec<PartAllocation>>>,
    /// Request counter for metrics
    request_count: AtomicU64,
    /// Cache hit counter
    cache_hits: AtomicU64,
}

impl MetaClient {
    /// Create a new Meta client
    pub async fn new(addr: &str) -> Result<Self> {
        let config = MetaClientConfig {
            addrs: vec![addr.to_string()],
            ..Default::default()
        };
        Self::with_config(config).await
    }

    /// Create a new Meta client with custom configuration
    pub async fn with_config(config: MetaClientConfig) -> Result<Self> {
        let client = Self {
            client: RwLock::new(None),
            config,
            leader: RwLock::new(None),
            space_cache: DashMap::new(),
            tag_cache: DashMap::new(),
            edge_cache: DashMap::new(),
            parts_cache: DashMap::new(),
            request_count: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
        };

        // Try to connect
        client.connect().await?;

        Ok(client)
    }

    /// Connect to the Meta service
    async fn connect(&self) -> Result<()> {
        let addrs = &self.config.addrs;

        for addr in addrs {
            let endpoint = format!("http://{}", addr);
            debug!("Attempting to connect to Meta service at {}", endpoint);

            match Endpoint::from_shared(endpoint.clone())
                .map_err(|e| MetaError::Internal(e.to_string()))?
                .connect_timeout(self.config.connect_timeout)
                .timeout(self.config.request_timeout)
                .connect()
                .await
            {
                Ok(channel) => {
                    let grpc_client = MetaServiceClient::new(channel);
                    *self.client.write().await = Some(grpc_client);
                    info!("Connected to Meta service at {}", addr);
                    return Ok(());
                }
                Err(e) => {
                    warn!("Failed to connect to {}: {}", addr, e);
                    continue;
                }
            }
        }

        Err(MetaError::Internal(
            "Failed to connect to any Meta service".to_string(),
        ))
    }

    /// Reconnect to the Meta service (possibly to a new leader)
    async fn reconnect(&self) -> Result<()> {
        // Try leader first if known
        if let Some(leader) = self.leader.read().await.as_ref() {
            let addr = format!("{}:{}", leader.host, leader.port);
            let endpoint = format!("http://{}", addr);

            if let Ok(channel) = Endpoint::from_shared(endpoint)
                .map_err(|e| MetaError::Internal(e.to_string()))?
                .connect_timeout(self.config.connect_timeout)
                .timeout(self.config.request_timeout)
                .connect()
                .await
            {
                let grpc_client = MetaServiceClient::new(channel);
                *self.client.write().await = Some(grpc_client);
                info!("Reconnected to Meta leader at {}", addr);
                return Ok(());
            }
        }

        // Fall back to configured addresses
        self.connect().await
    }

    /// Execute a request with retry logic
    async fn with_retry<T, F, Fut>(&self, operation: F) -> Result<T>
    where
        F: Fn(MetaServiceClient<Channel>) -> Fut,
        Fut: std::future::Future<Output = std::result::Result<tonic::Response<T>, tonic::Status>>,
        T: ResponseWithLeader,
    {
        self.request_count.fetch_add(1, Ordering::Relaxed);

        let mut last_error = None;
        let mut delay = self.config.retry_base_delay;

        for attempt in 0..=self.config.max_retries {
            if attempt > 0 {
                debug!("Retry attempt {} after {:?}", attempt, delay);
                tokio::time::sleep(delay).await;
                delay *= 2; // Exponential backoff

                // Try to reconnect before retry
                if let Err(e) = self.reconnect().await {
                    warn!("Reconnect failed: {}", e);
                    last_error = Some(MetaError::Internal(e.to_string()));
                    continue;
                }
            }

            let client = {
                let client_guard = self.client.read().await;
                match client_guard.as_ref() {
                    Some(c) => c.clone(),
                    None => {
                        drop(client_guard);
                        if let Err(e) = self.reconnect().await {
                            last_error = Some(MetaError::Internal(e.to_string()));
                            continue;
                        }
                        let client_guard = self.client.read().await;
                        match client_guard.as_ref() {
                            Some(c) => c.clone(),
                            None => continue,
                        }
                    }
                }
            };

            match operation(client).await {
                Ok(response) => {
                    let inner = response.into_inner();

                    // Update leader if provided
                    if let Some(leader) = inner.get_leader() {
                        *self.leader.write().await = Some(leader.clone());
                    }

                    // Check error code
                    let code = inner.get_code();
                    if code == ErrorCode::Succeeded as i32 {
                        return Ok(inner);
                    } else if code == ErrorCode::ELeaderChanged as i32 {
                        // Leader changed, retry with new leader
                        warn!("Leader changed, retrying...");
                        last_error = Some(MetaError::Internal("Leader changed".to_string()));
                        continue;
                    } else {
                        // Convert error code to MetaError
                        return Err(Self::code_to_error(code));
                    }
                }
                Err(status) => {
                    warn!("RPC failed: {}", status);
                    last_error = Some(MetaError::Internal(status.to_string()));
                }
            }
        }

        Err(last_error.unwrap_or_else(|| MetaError::Internal("Unknown error".to_string())))
    }

    fn code_to_error(code: i32) -> MetaError {
        match ErrorCode::try_from(code) {
            Ok(ErrorCode::ESpaceNotFound) => MetaError::SpaceNotFound("".to_string()),
            Ok(ErrorCode::ESpaceAlreadyExists) => MetaError::SpaceAlreadyExists("".to_string()),
            Ok(ErrorCode::ETagNotFound) => MetaError::TagNotFound("".to_string()),
            Ok(ErrorCode::ETagAlreadyExists) => MetaError::TagAlreadyExists("".to_string()),
            Ok(ErrorCode::EEdgeNotFound) => MetaError::EdgeNotFound("".to_string()),
            Ok(ErrorCode::EEdgeAlreadyExists) => MetaError::EdgeAlreadyExists("".to_string()),
            Ok(ErrorCode::EIndexNotFound) => MetaError::IndexNotFound("".to_string()),
            Ok(ErrorCode::EIndexAlreadyExists) => MetaError::IndexAlreadyExists("".to_string()),
            Ok(ErrorCode::EFieldNotFound) => MetaError::FieldNotFound("".to_string()),
            Ok(ErrorCode::EPartitionNotFound) => MetaError::PartitionNotFound(0, 0),
            _ => MetaError::Internal(format!("Error code: {}", code)),
        }
    }

    // ===== Space Operations =====

    /// Get space by name (cached)
    pub async fn get_space(&self, name: &str) -> Result<Space> {
        // Check cache first
        if let Some(entry) = self.space_cache.get(name) {
            if let Some(space) = entry.get() {
                self.cache_hits.fetch_add(1, Ordering::Relaxed);
                return Ok(space);
            }
        }

        let response = self
            .with_retry(|mut client| {
                let req = GetSpaceRequest {
                    space_name: name.to_string(),
                };
                async move { client.get_space(req).await }
            })
            .await?;

        let space_desc = response
            .space
            .ok_or_else(|| MetaError::SpaceNotFound(name.to_string()))?;
        let space = Self::convert_space_desc(&space_desc);

        // Update cache
        self.space_cache.insert(
            name.to_string(),
            CacheEntry::new(space.clone(), self.config.cache_ttl),
        );

        Ok(space)
    }

    /// List all spaces
    pub async fn list_spaces(&self) -> Result<Vec<Space>> {
        let response = self
            .with_retry(|mut client| {
                let req = ListSpacesRequest {};
                async move { client.list_spaces(req).await }
            })
            .await?;

        let spaces: Vec<Space> = response
            .spaces
            .iter()
            .map(Self::convert_space_desc)
            .collect();

        // Update cache
        for space in &spaces {
            self.space_cache.insert(
                space.name.clone(),
                CacheEntry::new(space.clone(), self.config.cache_ttl),
            );
        }

        Ok(spaces)
    }

    /// Create a new space
    pub async fn create_space(
        &self,
        name: &str,
        partition_num: u32,
        replica_factor: u32,
        vid_type: SchemaVidType,
    ) -> Result<u32> {
        self.create_space_with_strategy(
            name,
            partition_num,
            replica_factor,
            vid_type,
            byoridb_common::PartitionStrategy::Hash,
        )
        .await
    }

    /// Create a new space with a specific partition strategy
    pub async fn create_space_with_strategy(
        &self,
        name: &str,
        partition_num: u32,
        replica_factor: u32,
        vid_type: SchemaVidType,
        partition_strategy: byoridb_common::PartitionStrategy,
    ) -> Result<u32> {
        let (proto_vid_type, vid_size) = match vid_type {
            SchemaVidType::Int64 => (VidType::VidInt64, 0),
            SchemaVidType::FixedString(len) => (VidType::VidFixedString, len as u32),
        };

        let proto_strategy = Self::convert_partition_strategy_to_proto(&partition_strategy);

        let response = self
            .with_retry(|mut client| {
                let req = CreateSpaceRequest {
                    space_name: name.to_string(),
                    partition_num,
                    replica_factor,
                    vid_type: proto_vid_type as i32,
                    vid_size,
                    partition_strategy: Some(proto_strategy.clone()),
                };
                async move { client.create_space(req).await }
            })
            .await?;

        // Invalidate cache
        self.space_cache.remove(name);

        Ok(response.space_id)
    }

    /// Convert byoridb_common::PartitionStrategy to proto PartitionStrategy
    fn convert_partition_strategy_to_proto(
        strategy: &byoridb_common::PartitionStrategy,
    ) -> PartitionStrategy {
        match strategy {
            byoridb_common::PartitionStrategy::Hash => PartitionStrategy {
                strategy_type: PartitionStrategyType::PsHash as i32,
                range_boundaries: vec![],
            },
            byoridb_common::PartitionStrategy::Range { boundaries } => PartitionStrategy {
                strategy_type: PartitionStrategyType::PsRange as i32,
                range_boundaries: boundaries.clone(),
            },
            byoridb_common::PartitionStrategy::Modulo => PartitionStrategy {
                strategy_type: PartitionStrategyType::PsModulo as i32,
                range_boundaries: vec![],
            },
        }
    }

    /// Drop a space
    pub async fn drop_space(&self, name: &str) -> Result<()> {
        self.with_retry(|mut client| {
            let req = DropSpaceRequest {
                space_name: name.to_string(),
            };
            async move { client.drop_space(req).await }
        })
        .await?;

        // Invalidate cache
        self.space_cache.remove(name);

        Ok(())
    }

    // ===== Tag Operations =====

    /// Get tag by name (cached)
    pub async fn get_tag(&self, space_id: u32, name: &str) -> Result<TagSchema> {
        let cache_key = (space_id, name.to_string());

        // Check cache first
        if let Some(entry) = self.tag_cache.get(&cache_key) {
            if let Some(tag) = entry.get() {
                self.cache_hits.fetch_add(1, Ordering::Relaxed);
                return Ok(tag);
            }
        }

        let response = self
            .with_retry(|mut client| {
                let req = GetTagRequest {
                    space_id,
                    tag_name: name.to_string(),
                };
                async move { client.get_tag(req).await }
            })
            .await?;

        let tag_desc = response
            .tag
            .ok_or_else(|| MetaError::TagNotFound(name.to_string()))?;
        let tag = Self::convert_tag_desc(&tag_desc);

        // Update cache
        self.tag_cache.insert(
            cache_key,
            CacheEntry::new(tag.clone(), self.config.cache_ttl),
        );

        Ok(tag)
    }

    /// List all tags in a space
    pub async fn list_tags(&self, space_id: u32) -> Result<Vec<TagSchema>> {
        let response = self
            .with_retry(|mut client| {
                let req = ListTagsRequest { space_id };
                async move { client.list_tags(req).await }
            })
            .await?;

        let tags: Vec<TagSchema> = response.tags.iter().map(Self::convert_tag_desc).collect();

        // Update cache
        for tag in &tags {
            let cache_key = (space_id, tag.name.clone());
            self.tag_cache.insert(
                cache_key,
                CacheEntry::new(tag.clone(), self.config.cache_ttl),
            );
        }

        Ok(tags)
    }

    /// Create a new tag
    pub async fn create_tag(
        &self,
        space_id: u32,
        name: &str,
        fields: Vec<SchemaField>,
    ) -> Result<u32> {
        let proto_fields = Self::convert_fields_to_proto(&fields);

        let response = self
            .with_retry(|mut client| {
                let req = CreateTagRequest {
                    space_id,
                    tag_name: name.to_string(),
                    fields: proto_fields.clone(),
                };
                async move { client.create_tag(req).await }
            })
            .await?;

        // Invalidate cache
        self.tag_cache.remove(&(space_id, name.to_string()));

        Ok(response.tag_id)
    }

    /// Drop a tag
    pub async fn drop_tag(&self, space_id: u32, name: &str) -> Result<()> {
        self.with_retry(|mut client| {
            let req = DropTagRequest {
                space_id,
                tag_name: name.to_string(),
            };
            async move { client.drop_tag(req).await }
        })
        .await?;

        // Invalidate cache
        self.tag_cache.remove(&(space_id, name.to_string()));

        Ok(())
    }

    /// Alter a tag schema (add columns)
    ///
    /// Returns the new version number after the ALTER operation.
    pub async fn alter_tag(
        &self,
        space_id: u32,
        name: &str,
        operations: Vec<crate::schema::AlterOperation>,
    ) -> Result<i32> {
        let proto_ops = Self::convert_alter_ops_to_proto(&operations);

        let response = self
            .with_retry(|mut client| {
                let req = AlterTagRequest {
                    space_id,
                    tag_name: name.to_string(),
                    operations: proto_ops.clone(),
                };
                async move { client.alter_tag(req).await }
            })
            .await?;

        // Invalidate cache immediately after ALTER
        // WARNING: This only invalidates the cache on this client instance.
        // In a distributed environment, other clients may still serve stale schema data
        // until their TTL expires or they explicitly invalidate.
        self.tag_cache.remove(&(space_id, name.to_string()));

        Ok(response.new_version)
    }

    /// Get all versions of a tag schema
    pub async fn get_tag_versions(&self, space_id: u32, name: &str) -> Result<Vec<TagSchema>> {
        let response = self
            .with_retry(|mut client| {
                let req = GetTagVersionsRequest {
                    space_id,
                    tag_name: name.to_string(),
                };
                async move { client.get_tag_versions(req).await }
            })
            .await?;

        let versions: Vec<TagSchema> = response
            .versions
            .iter()
            .map(Self::convert_tag_desc)
            .collect();

        Ok(versions)
    }

    // ===== Edge Operations =====

    /// Get edge by name (cached)
    pub async fn get_edge(&self, space_id: u32, name: &str) -> Result<EdgeSchema> {
        let cache_key = (space_id, name.to_string());

        // Check cache first
        if let Some(entry) = self.edge_cache.get(&cache_key) {
            if let Some(edge) = entry.get() {
                self.cache_hits.fetch_add(1, Ordering::Relaxed);
                return Ok(edge);
            }
        }

        let response = self
            .with_retry(|mut client| {
                let req = GetEdgeRequest {
                    space_id,
                    edge_name: name.to_string(),
                };
                async move { client.get_edge(req).await }
            })
            .await?;

        let edge_desc = response
            .edge
            .ok_or_else(|| MetaError::EdgeNotFound(name.to_string()))?;
        let edge = Self::convert_edge_desc(&edge_desc);

        // Update cache
        self.edge_cache.insert(
            cache_key,
            CacheEntry::new(edge.clone(), self.config.cache_ttl),
        );

        Ok(edge)
    }

    /// List all edges in a space
    pub async fn list_edges(&self, space_id: u32) -> Result<Vec<EdgeSchema>> {
        let response = self
            .with_retry(|mut client| {
                let req = ListEdgesRequest { space_id };
                async move { client.list_edges(req).await }
            })
            .await?;

        let edges: Vec<EdgeSchema> = response.edges.iter().map(Self::convert_edge_desc).collect();

        // Update cache
        for edge in &edges {
            let cache_key = (space_id, edge.name.clone());
            self.edge_cache.insert(
                cache_key,
                CacheEntry::new(edge.clone(), self.config.cache_ttl),
            );
        }

        Ok(edges)
    }

    /// Alter an edge schema (add columns)
    ///
    /// Returns the new version number after the ALTER operation.
    pub async fn alter_edge(
        &self,
        space_id: u32,
        name: &str,
        operations: Vec<crate::schema::AlterOperation>,
    ) -> Result<i32> {
        let proto_ops = Self::convert_alter_ops_to_proto(&operations);

        let response = self
            .with_retry(|mut client| {
                let req = AlterEdgeRequest {
                    space_id,
                    edge_name: name.to_string(),
                    operations: proto_ops.clone(),
                };
                async move { client.alter_edge(req).await }
            })
            .await?;

        // Invalidate cache immediately after ALTER
        // WARNING: This only invalidates the cache on this client instance.
        // In a distributed environment, other clients may still serve stale schema data
        // until their TTL expires or they explicitly invalidate.
        self.edge_cache.remove(&(space_id, name.to_string()));

        Ok(response.new_version)
    }

    /// Get all versions of an edge schema
    pub async fn get_edge_versions(&self, space_id: u32, name: &str) -> Result<Vec<EdgeSchema>> {
        let response = self
            .with_retry(|mut client| {
                let req = GetEdgeVersionsRequest {
                    space_id,
                    edge_name: name.to_string(),
                };
                async move { client.get_edge_versions(req).await }
            })
            .await?;

        let versions: Vec<EdgeSchema> = response
            .versions
            .iter()
            .map(Self::convert_edge_desc)
            .collect();

        Ok(versions)
    }

    // ===== Tag Index Operations =====

    /// Create a new tag index
    ///
    /// Returns the generated index ID on success.
    pub async fn create_tag_index(
        &self,
        space_id: u32,
        index_name: &str,
        tag_name: &str,
        fields: Vec<String>,
    ) -> Result<u32> {
        let response = self
            .with_retry(|mut client| {
                let req = CreateTagIndexRequest {
                    space_id,
                    index_name: index_name.to_string(),
                    tag_name: tag_name.to_string(),
                    fields: fields.clone(),
                };
                async move { client.create_tag_index(req).await }
            })
            .await?;

        Ok(response.index_id)
    }

    /// List tag indexes in a space
    pub async fn list_tag_indexes(&self, space_id: u32) -> Result<Vec<TagIndex>> {
        let response = self
            .with_retry(|mut client| {
                let req = ListTagIndexesRequest { space_id };
                async move { client.list_tag_indexes(req).await }
            })
            .await?;

        let indexes: Vec<TagIndex> = response
            .indexes
            .iter()
            .map(|i| TagIndex {
                id: i.index_id,
                space_id: i.space_id,
                index_name: i.index_name.clone(),
                tag_id: i.tag_id,
                fields: i.fields.clone(),
            })
            .collect();

        Ok(indexes)
    }

    /// Drop a tag index by name
    pub async fn drop_tag_index(&self, space_id: u32, index_name: &str) -> Result<()> {
        self.with_retry(|mut client| {
            let req = DropTagIndexRequest {
                space_id,
                index_name: index_name.to_string(),
            };
            async move { client.drop_tag_index(req).await }
        })
        .await?;

        Ok(())
    }

    // ===== Edge Index Operations =====

    /// Create a new edge index
    ///
    /// Returns the generated index ID on success.
    pub async fn create_edge_index(
        &self,
        space_id: u32,
        index_name: &str,
        edge_name: &str,
        fields: Vec<String>,
    ) -> Result<u32> {
        let response = self
            .with_retry(|mut client| {
                let req = CreateEdgeIndexRequest {
                    space_id,
                    index_name: index_name.to_string(),
                    edge_name: edge_name.to_string(),
                    fields: fields.clone(),
                };
                async move { client.create_edge_index(req).await }
            })
            .await?;

        Ok(response.index_id)
    }

    /// List edge indexes in a space
    pub async fn list_edge_indexes(&self, space_id: u32) -> Result<Vec<EdgeIndex>> {
        let response = self
            .with_retry(|mut client| {
                let req = ListEdgeIndexesRequest { space_id };
                async move { client.list_edge_indexes(req).await }
            })
            .await?;

        let indexes: Vec<EdgeIndex> = response
            .indexes
            .iter()
            .map(|i| EdgeIndex {
                id: i.index_id,
                space_id: i.space_id,
                index_name: i.index_name.clone(),
                edge_type: i.edge_id,
                fields: i.fields.clone(),
            })
            .collect();

        Ok(indexes)
    }

    /// Drop an edge index by name
    pub async fn drop_edge_index(&self, space_id: u32, index_name: &str) -> Result<()> {
        self.with_retry(|mut client| {
            let req = DropEdgeIndexRequest {
                space_id,
                index_name: index_name.to_string(),
            };
            async move { client.drop_edge_index(req).await }
        })
        .await?;

        Ok(())
    }

    // ===== Partition Operations =====

    /// Get partition allocation for a space (cached)
    pub async fn get_parts_alloc(&self, space_id: u32) -> Result<Vec<PartAllocation>> {
        // Check cache first
        if let Some(entry) = self.parts_cache.get(&space_id) {
            if let Some(parts) = entry.get() {
                self.cache_hits.fetch_add(1, Ordering::Relaxed);
                return Ok(parts);
            }
        }

        let response = self
            .with_retry(|mut client| {
                let req = GetPartsAllocRequest { space_id };
                async move { client.get_parts_alloc(req).await }
            })
            .await?;

        let parts: Vec<PartAllocation> = response
            .parts
            .iter()
            .map(|p| PartAllocation {
                space_id: p.space_id,
                part_id: p.part_id,
                hosts: p.hosts.iter().map(|h| (h.host.clone(), h.port)).collect(),
            })
            .collect();

        // Update cache
        self.parts_cache.insert(
            space_id,
            CacheEntry::new(parts.clone(), self.config.cache_ttl),
        );

        Ok(parts)
    }

    /// Get hosts for a specific partition
    pub async fn get_part_hosts(&self, space_id: u32, part_id: u32) -> Result<Vec<(String, u32)>> {
        let response = self
            .with_retry(|mut client| {
                let req = GetPartHostsRequest { space_id, part_id };
                async move { client.get_part_hosts(req).await }
            })
            .await?;

        let hosts: Vec<(String, u32)> = response
            .hosts
            .iter()
            .map(|h| (h.host.clone(), h.port))
            .collect();

        Ok(hosts)
    }

    // ===== Host Operations =====

    /// List all registered storage hosts with live leader / partition counts.
    ///
    /// Returns every host known to the Meta service, including those whose
    /// heartbeat has lapsed (they are reported as [`HostStatus::Offline`]).
    /// This is intended for the `SHOW HOSTS` admin command; it is not cached
    /// because the liveness of hosts changes on every heartbeat tick.
    pub async fn list_hosts(&self) -> Result<Vec<HostInfo>> {
        let response = self
            .with_retry(|mut client| {
                let req = ListHostsRequest {};
                async move { client.list_hosts(req).await }
            })
            .await?;

        let hosts = response
            .hosts
            .into_iter()
            .map(|item| {
                let addr = item.host.unwrap_or(HostAddr {
                    host: String::new(),
                    port: 0,
                });
                let status = match HostStatusProto::try_from(item.status)
                    .unwrap_or(HostStatusProto::HsOffline)
                {
                    HostStatusProto::HsOnline => HostLiveness::Online,
                    HostStatusProto::HsOffline => HostLiveness::Offline,
                };
                HostInfo {
                    host: addr.host,
                    port: addr.port,
                    status,
                    leader_count: item.leader_count,
                    part_count: item.part_count,
                }
            })
            .collect();

        Ok(hosts)
    }

    // ===== Heartbeat =====

    /// Send heartbeat to Meta service
    ///
    /// Used by storage nodes to register themselves with the Meta service.
    /// This enables the Meta service to track available storage hosts for
    /// partition allocation.
    ///
    /// # Arguments
    /// * `host` - This node's hostname
    /// * `port` - This node's port
    /// * `role` - Node role: "storage", "graph", or "meta"
    ///
    /// # Returns
    /// The cluster ID on success
    pub async fn send_heartbeat(&self, host: &str, port: u32, role: &str) -> Result<i64> {
        // On first heartbeat cluster_id is 0 (unknown). Subsequent calls
        // should pass the cluster_id received from the previous response.
        self.send_heartbeat_with_cluster_id(host, port, role, 0)
            .await
    }

    /// Send heartbeat with a known cluster_id for validation.
    pub async fn send_heartbeat_with_cluster_id(
        &self,
        host: &str,
        port: u32,
        role: &str,
        cluster_id: i64,
    ) -> Result<i64> {
        let response = self
            .with_retry(|mut client| {
                let req = HeartbeatRequest {
                    host: Some(HostAddr {
                        host: host.to_string(),
                        port,
                    }),
                    role: role.to_string(),
                    cluster_id,
                };
                async move { client.heartbeat(req).await }
            })
            .await?;

        Ok(response.cluster_id)
    }

    // ===== Cache Management =====

    /// Invalidate all caches
    pub fn invalidate_cache(&self) {
        self.space_cache.clear();
        self.tag_cache.clear();
        self.edge_cache.clear();
        self.parts_cache.clear();
    }

    /// Get cache statistics
    pub fn cache_stats(&self) -> (u64, u64) {
        (
            self.request_count.load(Ordering::Relaxed),
            self.cache_hits.load(Ordering::Relaxed),
        )
    }

    // ===== Type Conversion Helpers =====

    fn convert_space_desc(desc: &SpaceDesc) -> Space {
        Space {
            id: desc.space_id,
            name: desc.space_name.clone(),
            partition_num: desc.partition_num,
            replica_factor: desc.replica_factor,
            vid_type: match VidType::try_from(desc.vid_type) {
                Ok(VidType::VidFixedString) => SchemaVidType::FixedString(desc.vid_size as usize),
                _ => SchemaVidType::Int64,
            },
            partition_strategy: Self::convert_partition_strategy_from_proto(
                desc.partition_strategy.as_ref(),
            ),
        }
    }

    /// Convert proto PartitionStrategy to byoridb_common::PartitionStrategy
    fn convert_partition_strategy_from_proto(
        strategy: Option<&PartitionStrategy>,
    ) -> byoridb_common::PartitionStrategy {
        match strategy {
            Some(s) => {
                let strategy_type = PartitionStrategyType::try_from(s.strategy_type)
                    .unwrap_or(PartitionStrategyType::PsHash);
                match strategy_type {
                    PartitionStrategyType::PsHash => byoridb_common::PartitionStrategy::Hash,
                    PartitionStrategyType::PsRange => byoridb_common::PartitionStrategy::Range {
                        boundaries: s.range_boundaries.clone(),
                    },
                    PartitionStrategyType::PsModulo => byoridb_common::PartitionStrategy::Modulo,
                }
            }
            None => byoridb_common::PartitionStrategy::Hash,
        }
    }

    fn convert_tag_desc(desc: &TagDesc) -> TagSchema {
        TagSchema {
            id: desc.tag_id,
            space_id: desc.space_id,
            name: desc.tag_name.clone(),
            version: desc.version,
            fields: Self::convert_fields_from_proto(&desc.fields),
        }
    }

    fn convert_edge_desc(desc: &EdgeDesc) -> EdgeSchema {
        EdgeSchema {
            id: desc.edge_id,
            space_id: desc.space_id,
            name: desc.edge_name.clone(),
            version: desc.version,
            fields: Self::convert_fields_from_proto(&desc.fields),
        }
    }

    fn convert_fields_from_proto(fields: &[Field]) -> Vec<SchemaField> {
        fields
            .iter()
            .map(|f| SchemaField {
                name: f.name.clone(),
                data_type: Self::convert_data_type_from_proto(f.data_type, f.fixed_string_len),
                nullable: f.nullable,
                default: if f.default_value.is_empty() {
                    None
                } else {
                    Some(f.default_value.clone())
                },
            })
            .collect()
    }

    fn convert_fields_to_proto(fields: &[SchemaField]) -> Vec<Field> {
        fields
            .iter()
            .map(|f| {
                let (data_type, fixed_len) = Self::convert_data_type_to_proto(&f.data_type);
                Field {
                    name: f.name.clone(),
                    data_type: data_type as i32,
                    nullable: f.nullable,
                    default_value: f.default.clone().unwrap_or_default(),
                    fixed_string_len: fixed_len,
                }
            })
            .collect()
    }

    fn convert_data_type_from_proto(dt: i32, fixed_len: u32) -> SchemaDataType {
        match DataType::try_from(dt) {
            Ok(DataType::DtBool) => SchemaDataType::Bool,
            Ok(DataType::DtInt8) => SchemaDataType::Int8,
            Ok(DataType::DtInt16) => SchemaDataType::Int16,
            Ok(DataType::DtInt32) => SchemaDataType::Int32,
            Ok(DataType::DtInt64) => SchemaDataType::Int64,
            Ok(DataType::DtFloat) => SchemaDataType::Float,
            Ok(DataType::DtDouble) => SchemaDataType::Double,
            Ok(DataType::DtString) => SchemaDataType::String,
            Ok(DataType::DtFixedString) => SchemaDataType::FixedString(fixed_len as usize),
            Ok(DataType::DtTimestamp) => SchemaDataType::Timestamp,
            Ok(DataType::DtDate) => SchemaDataType::Date,
            Ok(DataType::DtTime) => SchemaDataType::Time,
            Ok(DataType::DtDatetime) => SchemaDataType::DateTime,
            Ok(DataType::DtGeography) => SchemaDataType::Geography,
            _ => SchemaDataType::String,
        }
    }

    fn convert_data_type_to_proto(dt: &SchemaDataType) -> (DataType, u32) {
        match dt {
            SchemaDataType::Bool => (DataType::DtBool, 0),
            SchemaDataType::Int8 => (DataType::DtInt8, 0),
            SchemaDataType::Int16 => (DataType::DtInt16, 0),
            SchemaDataType::Int32 => (DataType::DtInt32, 0),
            SchemaDataType::Int64 => (DataType::DtInt64, 0),
            SchemaDataType::Float => (DataType::DtFloat, 0),
            SchemaDataType::Double => (DataType::DtDouble, 0),
            SchemaDataType::String => (DataType::DtString, 0),
            SchemaDataType::FixedString(len) => (DataType::DtFixedString, *len as u32),
            SchemaDataType::Timestamp => (DataType::DtTimestamp, 0),
            SchemaDataType::Date => (DataType::DtDate, 0),
            SchemaDataType::Time => (DataType::DtTime, 0),
            SchemaDataType::DateTime => (DataType::DtDatetime, 0),
            SchemaDataType::Geography => (DataType::DtGeography, 0),
        }
    }

    fn convert_alter_ops_to_proto(
        operations: &[crate::schema::AlterOperation],
    ) -> Vec<AlterOperation> {
        operations
            .iter()
            .map(|op| match op {
                crate::schema::AlterOperation::AddColumn(field) => {
                    let (data_type, fixed_len) = Self::convert_data_type_to_proto(&field.data_type);
                    AlterOperation {
                        op_type: AlterOperationType::AddColumn as i32,
                        field: Some(Field {
                            name: field.name.clone(),
                            data_type: data_type as i32,
                            nullable: field.nullable,
                            default_value: field.default.clone().unwrap_or_default(),
                            fixed_string_len: fixed_len,
                        }),
                        field_name: String::new(),
                    }
                }
                crate::schema::AlterOperation::DropColumn(col_name) => AlterOperation {
                    op_type: AlterOperationType::DropColumn as i32,
                    field: None,
                    field_name: col_name.clone(),
                },
                crate::schema::AlterOperation::ChangeColumn(field) => {
                    let (data_type, fixed_len) = Self::convert_data_type_to_proto(&field.data_type);
                    AlterOperation {
                        op_type: AlterOperationType::ChangeColumn as i32,
                        field: Some(Field {
                            name: field.name.clone(),
                            data_type: data_type as i32,
                            nullable: field.nullable,
                            default_value: field.default.clone().unwrap_or_default(),
                            fixed_string_len: fixed_len,
                        }),
                        field_name: String::new(),
                    }
                }
            })
            .collect()
    }
}

/// Trait for responses that include a leader hint
trait ResponseWithLeader {
    fn get_code(&self) -> i32;
    fn get_leader(&self) -> Option<&HostAddr>;
}

macro_rules! impl_response_with_leader {
    ($($type:ty),*) => {
        $(
            impl ResponseWithLeader for $type {
                fn get_code(&self) -> i32 {
                    self.code
                }
                fn get_leader(&self) -> Option<&HostAddr> {
                    self.leader.as_ref()
                }
            }
        )*
    };
}

impl_response_with_leader!(
    CreateSpaceResponse,
    GetSpaceResponse,
    ListSpacesResponse,
    DropSpaceResponse,
    CreateTagResponse,
    GetTagResponse,
    ListTagsResponse,
    DropTagResponse,
    AlterTagResponse,
    GetTagVersionsResponse,
    CreateEdgeResponse,
    GetEdgeResponse,
    ListEdgesResponse,
    DropEdgeResponse,
    AlterEdgeResponse,
    GetEdgeVersionsResponse,
    CreateTagIndexResponse,
    ListTagIndexesResponse,
    DropTagIndexResponse,
    CreateEdgeIndexResponse,
    ListEdgeIndexesResponse,
    DropEdgeIndexResponse,
    GetPartsAllocResponse,
    GetPartHostsResponse,
    ListHostsResponse,
    HeartbeatResponse
);

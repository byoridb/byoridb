// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Storage RPC service implementation for data migration and partition management

use crate::codec::Codec;
use crate::index::{IndexManager, IndexType};
use crate::key::IndexValue as StorageIndexValue;
use crate::partition::{PartitionManager, PartitionStatus as LocalPartitionStatus};
use crate::proto::storage::storage_service_server::StorageService;
use crate::proto::storage::{
    BatchGetEdgesRequest, BatchGetEdgesResponse, BatchGetVerticesRequest, BatchGetVerticesResponse,
    BloomFilterType, CheckBloomFilterRequest, CheckBloomFilterResponse, EdgeData, EdgeKey,
    ErrorCode, GetNeighborsBySourceRequest, GetNeighborsBySourceResponse, GetPartitionDataRequest,
    GetPartitionStatusRequest, GetPartitionStatusResponse, HostAddr, IndexValue as ProtoIndexValue,
    KeyValuePair, LookupEdgeIndexRequest, LookupEdgeIndexResponse, LookupTagIndexRequest,
    LookupTagIndexResponse, PartitionDataChunk, PartitionStatus as ProtoPartitionStatus,
    ScanEdgesRequest, ScanEdgesResponse, ScanPartitionRequest, ScanVerticesRequest,
    ScanVerticesResponse, TagData, TransferPartitionResponse, UpdatePartitionOwnershipRequest,
    UpdatePartitionOwnershipResponse, VertexData,
};
use byoridb_kvstore::KVStore;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::Stream;
use tokio_stream::StreamExt;
use tonic::{Request, Response, Status, Streaming};
use tracing::{debug, error, info, warn};

/// Compute checksum for a batch of key-value pairs
fn compute_batch_checksum(data: &[KeyValuePair]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for kv in data {
        kv.key.hash(&mut hasher);
        kv.value.hash(&mut hasher);
    }
    hasher.finish()
}

/// Storage RPC service implementation
pub struct StorageRpcService {
    partition_manager: Arc<PartitionManager>,
    kvstore: Arc<dyn KVStore>,
    index_manager: Arc<IndexManager>,
}

impl StorageRpcService {
    pub fn new(partition_manager: Arc<PartitionManager>, kvstore: Arc<dyn KVStore>) -> Self {
        let index_manager = Arc::new(IndexManager::new(kvstore.clone()));
        Self {
            partition_manager,
            kvstore,
            index_manager,
        }
    }

    /// Create with explicit IndexManager (for testing or custom configuration)
    pub fn with_index_manager(
        partition_manager: Arc<PartitionManager>,
        kvstore: Arc<dyn KVStore>,
        index_manager: Arc<IndexManager>,
    ) -> Self {
        Self {
            partition_manager,
            kvstore,
            index_manager,
        }
    }

    /// Convert proto IndexValue to storage IndexValue
    fn convert_proto_index_value(value: &ProtoIndexValue) -> Option<StorageIndexValue> {
        use crate::proto::storage::index_value::Value;
        match &value.value {
            Some(Value::BoolValue(b)) => Some(StorageIndexValue::Bool(*b)),
            Some(Value::IntValue(i)) => Some(StorageIndexValue::Int(*i)),
            Some(Value::FloatValue(f)) => Some(StorageIndexValue::Float(*f)),
            Some(Value::StringValue(s)) => Some(StorageIndexValue::String(s.clone())),
            None => Some(StorageIndexValue::Null),
        }
    }

    fn convert_status_to_proto(status: &LocalPartitionStatus) -> ProtoPartitionStatus {
        match status {
            LocalPartitionStatus::Leader => ProtoPartitionStatus::PsLeader,
            LocalPartitionStatus::Follower => ProtoPartitionStatus::PsFollower,
            LocalPartitionStatus::NotOwned => ProtoPartitionStatus::PsNotOwned,
            LocalPartitionStatus::Transferring => ProtoPartitionStatus::PsTransferring,
        }
    }

    fn convert_proto_to_status(status: ProtoPartitionStatus) -> LocalPartitionStatus {
        match status {
            ProtoPartitionStatus::PsLeader => LocalPartitionStatus::Leader,
            ProtoPartitionStatus::PsFollower => LocalPartitionStatus::Follower,
            ProtoPartitionStatus::PsNotOwned => LocalPartitionStatus::NotOwned,
            ProtoPartitionStatus::PsTransferring => LocalPartitionStatus::Transferring,
        }
    }
}

#[tonic::async_trait]
impl StorageService for StorageRpcService {
    type GetPartitionDataStream =
        Pin<Box<dyn Stream<Item = Result<PartitionDataChunk, Status>> + Send>>;
    type ScanPartitionStream = Pin<Box<dyn Stream<Item = Result<KeyValuePair, Status>> + Send>>;

    /// Get all data for a partition (for migration)
    async fn get_partition_data(
        &self,
        request: Request<GetPartitionDataRequest>,
    ) -> Result<Response<Self::GetPartitionDataStream>, Status> {
        let req = request.into_inner();
        let space_id = req.space_id;
        let part_id = req.part_id;

        info!(
            "GetPartitionData request: space_id={}, part_id={}",
            space_id, part_id
        );

        // Verify we own this partition
        if !self
            .partition_manager
            .owns_partition(space_id, part_id)
            .await
        {
            return Err(Status::not_found(format!(
                "Partition {} not owned by this node",
                part_id
            )));
        }

        let kvstore = self.kvstore.clone();
        let key_prefix = if req.key_prefix.is_empty() {
            // Default prefix for partition data
            format!("p{}:s{}:", part_id, space_id).into_bytes()
        } else {
            req.key_prefix
        };

        let (tx, rx) = mpsc::channel(32);

        // Spawn task to stream partition data
        tokio::spawn(async move {
            let mut batch = Vec::new();
            const BATCH_SIZE: usize = 1000;

            // Scan all keys with the partition prefix
            match kvstore.scan_prefix(&key_prefix).await {
                Ok(pairs) => {
                    let total_keys = pairs.len() as u64;

                    for (key, value) in pairs {
                        batch.push(KeyValuePair { key, value });

                        if batch.len() >= BATCH_SIZE {
                            let data = std::mem::take(&mut batch);
                            let checksum = compute_batch_checksum(&data);
                            let chunk = PartitionDataChunk {
                                space_id,
                                part_id,
                                data,
                                done: false,
                                total_keys,
                                checksum,
                            };
                            if tx.send(Ok(chunk)).await.is_err() {
                                warn!("Client disconnected during partition data transfer");
                                return;
                            }
                        }
                    }

                    // Send final chunk
                    let checksum = compute_batch_checksum(&batch);
                    let chunk = PartitionDataChunk {
                        space_id,
                        part_id,
                        data: batch,
                        done: true,
                        total_keys,
                        checksum,
                    };
                    let _ = tx.send(Ok(chunk)).await;
                }
                Err(e) => {
                    error!("Failed to scan partition data: {}", e);
                    let _ = tx
                        .send(Err(Status::internal(format!(
                            "Failed to scan partition: {}",
                            e
                        ))))
                        .await;
                }
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    /// Receive partition data during migration
    async fn transfer_partition(
        &self,
        request: Request<Streaming<PartitionDataChunk>>,
    ) -> Result<Response<TransferPartitionResponse>, Status> {
        let mut stream = request.into_inner();
        let mut keys_received = 0u64;
        let mut space_id = 0u32;
        let mut part_id = 0u32;

        info!("TransferPartition: starting to receive data");

        while let Some(chunk_result) = stream.next().await {
            match chunk_result {
                Ok(chunk) => {
                    space_id = chunk.space_id;
                    part_id = chunk.part_id;

                    // Store each key-value pair
                    for kv in chunk.data {
                        if let Err(e) = self.kvstore.put(&kv.key, &kv.value).await {
                            error!("Failed to store key during migration: {}", e);
                            return Ok(Response::new(TransferPartitionResponse {
                                code: ErrorCode::ETransferFailed as i32,
                                keys_received,
                                error_message: format!("Failed to store key: {}", e),
                            }));
                        }
                        keys_received += 1;
                    }

                    if chunk.done {
                        info!(
                            "TransferPartition completed: space_id={}, part_id={}, keys={}",
                            space_id, part_id, keys_received
                        );
                        break;
                    }
                }
                Err(e) => {
                    error!("Error receiving partition data: {}", e);
                    return Ok(Response::new(TransferPartitionResponse {
                        code: ErrorCode::ETransferFailed as i32,
                        keys_received,
                        error_message: format!("Stream error: {}", e),
                    }));
                }
            }
        }

        // Update partition ownership to mark it as owned
        self.partition_manager
            .add_partition_with_status(space_id, part_id, LocalPartitionStatus::Follower)
            .await;

        Ok(Response::new(TransferPartitionResponse {
            code: ErrorCode::Succeeded as i32,
            keys_received,
            error_message: String::new(),
        }))
    }

    /// Update partition ownership (from Meta service)
    async fn update_partition_ownership(
        &self,
        request: Request<UpdatePartitionOwnershipRequest>,
    ) -> Result<Response<UpdatePartitionOwnershipResponse>, Status> {
        let req = request.into_inner();

        info!(
            "UpdatePartitionOwnership: space_id={}, part_id={}, status={:?}",
            req.space_id, req.part_id, req.new_status
        );

        let new_status = Self::convert_proto_to_status(
            ProtoPartitionStatus::try_from(req.new_status)
                .unwrap_or(ProtoPartitionStatus::PsNotOwned),
        );

        // Update local partition status
        if new_status == LocalPartitionStatus::NotOwned {
            // Remove partition from this node
            self.partition_manager
                .remove_partition(req.space_id, req.part_id)
                .await;
        } else {
            // Add or update partition
            if self
                .partition_manager
                .owns_partition(req.space_id, req.part_id)
                .await
            {
                self.partition_manager
                    .update_status(req.space_id, req.part_id, new_status)
                    .await;
            } else {
                self.partition_manager
                    .add_partition_with_status(req.space_id, req.part_id, new_status)
                    .await;
            }

            // Update leader info if provided
            if let Some(leader) = req.leader {
                self.partition_manager
                    .update_leader(req.space_id, req.part_id, Some((leader.host, leader.port)))
                    .await;
            }
        }

        Ok(Response::new(UpdatePartitionOwnershipResponse {
            code: ErrorCode::Succeeded as i32,
            error_message: String::new(),
        }))
    }

    /// Get partition status
    async fn get_partition_status(
        &self,
        request: Request<GetPartitionStatusRequest>,
    ) -> Result<Response<GetPartitionStatusResponse>, Status> {
        let req = request.into_inner();

        debug!(
            "GetPartitionStatus: space_id={}, part_id={}",
            req.space_id, req.part_id
        );

        let partition_info = self
            .partition_manager
            .get_partition(req.space_id, req.part_id)
            .await;

        match partition_info {
            Some(info) => {
                let leader = info.leader.map(|(host, port)| HostAddr { host, port });
                let replicas = info
                    .peers
                    .iter()
                    .map(|(host, port)| HostAddr {
                        host: host.clone(),
                        port: *port,
                    })
                    .collect();

                // Get key count and data size (simplified)
                let key_prefix = format!("p{}:s{}:", req.part_id, req.space_id).into_bytes();
                let (key_count, data_size_bytes) = match self.kvstore.scan_prefix(&key_prefix).await
                {
                    Ok(pairs) => {
                        let count = pairs.len() as u64;
                        let size: u64 = pairs.iter().map(|(k, v)| (k.len() + v.len()) as u64).sum();
                        (count, size)
                    }
                    Err(_) => (0, 0),
                };

                Ok(Response::new(GetPartitionStatusResponse {
                    code: ErrorCode::Succeeded as i32,
                    status: Self::convert_status_to_proto(&info.status) as i32,
                    leader,
                    replicas,
                    key_count,
                    data_size_bytes,
                }))
            }
            None => Ok(Response::new(GetPartitionStatusResponse {
                code: ErrorCode::EPartitionNotFound as i32,
                status: ProtoPartitionStatus::PsNotOwned as i32,
                leader: None,
                replicas: vec![],
                key_count: 0,
                data_size_bytes: 0,
            })),
        }
    }

    /// Scan partition keys with prefix
    async fn scan_partition(
        &self,
        request: Request<ScanPartitionRequest>,
    ) -> Result<Response<Self::ScanPartitionStream>, Status> {
        let req = request.into_inner();

        debug!(
            "ScanPartition: space_id={}, part_id={}",
            req.space_id, req.part_id
        );

        // Verify we own this partition
        if !self
            .partition_manager
            .owns_partition(req.space_id, req.part_id)
            .await
        {
            return Err(Status::not_found(format!(
                "Partition {} not owned by this node",
                req.part_id
            )));
        }

        let kvstore = self.kvstore.clone();
        let limit = if req.limit > 0 {
            Some(req.limit as usize)
        } else {
            None
        };

        let (tx, rx) = mpsc::channel(32);
        let end_key = req.end_key.clone();
        let start_key = req.start_key.clone();

        tokio::spawn(async move {
            // Create filter for end_key if provided
            let filter: byoridb_kvstore::FilterFn = if end_key.is_empty() {
                Box::new(|_, _| true)
            } else {
                let end = end_key.clone();
                Box::new(move |k: &[u8], _| k < end.as_slice())
            };

            match kvstore.scan_with_filter(&start_key, filter, limit).await {
                Ok(pairs) => {
                    for (key, value) in pairs {
                        if tx.send(Ok(KeyValuePair { key, value })).await.is_err() {
                            break;
                        }
                    }
                }
                Err(e) => {
                    let _ = tx
                        .send(Err(Status::internal(format!("Scan failed: {}", e))))
                        .await;
                }
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    // ===== Query RPCs =====

    /// Batch get vertices by VID list
    async fn batch_get_vertices(
        &self,
        request: Request<BatchGetVerticesRequest>,
    ) -> Result<Response<BatchGetVerticesResponse>, Status> {
        let req = request.into_inner();
        let space_id = req.space_id;

        debug!(
            "BatchGetVertices: space_id={}, vids_count={}",
            space_id,
            req.vids.len()
        );

        let mut vertices = Vec::new();

        // Build keys for batch lookup
        let keys: Vec<Vec<u8>> = req
            .vids
            .iter()
            .map(|vid| format!("{}:vertex:{}", space_id, vid).into_bytes())
            .collect();

        // Batch get from KVStore
        match self.kvstore.batch_get(&keys).await {
            Ok(results) => {
                for (vid, data_opt) in req.vids.iter().zip(results.iter()) {
                    if let Some(data) = data_opt {
                        // Use codec to decode vertex (supports both Proto and JSON formats)
                        if let Ok(vertex_data) = Codec::decode_vertex(data) {
                            let mut tag_data_list = Vec::new();

                            for tag in &vertex_data.tags {
                                // Filter by requested tag names if specified
                                if !req.tag_names.is_empty() && !req.tag_names.contains(&tag.name) {
                                    continue;
                                }

                                let mut properties: HashMap<String, Vec<u8>> = HashMap::new();

                                for (key, value) in &tag.properties {
                                    // Filter by requested property names if specified
                                    if !req.prop_names.is_empty() && !req.prop_names.contains(key) {
                                        continue;
                                    }
                                    // Encode property value using bincode
                                    if let Ok(bytes) = bincode::serialize(value) {
                                        properties.insert(key.clone(), bytes);
                                    }
                                }

                                tag_data_list.push(TagData {
                                    tag_name: tag.name.clone(),
                                    properties,
                                });
                            }

                            vertices.push(VertexData {
                                vid: *vid,
                                tags: tag_data_list,
                            });
                        }
                    }
                }

                Ok(Response::new(BatchGetVerticesResponse {
                    code: ErrorCode::Succeeded as i32,
                    vertices,
                    error_message: String::new(),
                }))
            }
            Err(e) => {
                error!("Failed to batch get vertices: {}", e);
                Ok(Response::new(BatchGetVerticesResponse {
                    code: ErrorCode::EInternalError as i32,
                    vertices: vec![],
                    error_message: format!("Failed to fetch vertices: {}", e),
                }))
            }
        }
    }

    /// Batch get edges by EdgeKey list
    async fn batch_get_edges(
        &self,
        request: Request<BatchGetEdgesRequest>,
    ) -> Result<Response<BatchGetEdgesResponse>, Status> {
        let req = request.into_inner();
        let space_id = req.space_id;

        debug!(
            "BatchGetEdges: space_id={}, edge_keys_count={}",
            space_id,
            req.edge_keys.len()
        );

        let mut edges = Vec::new();

        // Build keys for batch lookup
        let keys: Vec<Vec<u8>> = req
            .edge_keys
            .iter()
            .map(|ek| {
                format!(
                    "{}:edge:{}:{}:{}",
                    space_id, ek.src_vid, ek.edge_type, ek.ranking
                )
                .into_bytes()
            })
            .collect();

        // Batch get from KVStore
        match self.kvstore.batch_get(&keys).await {
            Ok(results) => {
                for (_edge_key, data_opt) in req.edge_keys.iter().zip(results.iter()) {
                    if let Some(data) = data_opt {
                        // Use codec to decode edge (supports both Proto and JSON formats)
                        if let Ok(edge_data) = Codec::decode_edge(data) {
                            let mut properties: HashMap<String, Vec<u8>> = HashMap::new();

                            for (key, value) in &edge_data.properties {
                                // Filter by requested property names if specified
                                if !req.prop_names.is_empty() && !req.prop_names.contains(key) {
                                    continue;
                                }
                                // Encode property value using bincode
                                if let Ok(bytes) = bincode::serialize(value) {
                                    properties.insert(key.clone(), bytes);
                                }
                            }

                            edges.push(EdgeData {
                                key: Some(EdgeKey {
                                    src_vid: edge_data.src_vid,
                                    dst_vid: edge_data.dst_vid,
                                    edge_type: edge_data.edge_type.clone(),
                                    ranking: edge_data.ranking,
                                }),
                                properties,
                            });
                        }
                    }
                }

                Ok(Response::new(BatchGetEdgesResponse {
                    code: ErrorCode::Succeeded as i32,
                    edges,
                    error_message: String::new(),
                }))
            }
            Err(e) => {
                error!("Failed to batch get edges: {}", e);
                Ok(Response::new(BatchGetEdgesResponse {
                    code: ErrorCode::EInternalError as i32,
                    edges: vec![],
                    error_message: format!("Failed to fetch edges: {}", e),
                }))
            }
        }
    }

    /// Scan vertices in a partition
    async fn scan_vertices(
        &self,
        request: Request<ScanVerticesRequest>,
    ) -> Result<Response<ScanVerticesResponse>, Status> {
        let req = request.into_inner();
        let space_id = req.space_id;
        let part_id = req.part_id;

        debug!(
            "ScanVertices: space_id={}, part_id={}, tag={}",
            space_id, part_id, req.tag_name
        );

        // Verify we own this partition
        if !self
            .partition_manager
            .owns_partition(space_id, part_id)
            .await
        {
            return Ok(Response::new(ScanVerticesResponse {
                code: ErrorCode::EPartitionNotFound as i32,
                vertices: vec![],
                next_cursor: vec![],
                has_more: false,
                error_message: format!("Partition {} not owned by this node", part_id),
            }));
        }

        // Build scan prefix for partition
        let prefix = format!("{}:vertex:", space_id);
        let limit = if req.limit > 0 {
            req.limit as usize
        } else {
            1000
        };

        // Determine start key (use cursor if provided)
        let start_key = if req.cursor.is_empty() {
            prefix.clone().into_bytes()
        } else {
            req.cursor.clone()
        };

        // Create filter to select only vertices in this partition
        let partition_num = self
            .partition_manager
            .get_partition_num(space_id)
            .await
            .unwrap_or(100);
        let tag_filter = req.tag_name.clone();

        let filter_fn: byoridb_kvstore::FilterFn = Box::new(move |key: &[u8], value: &[u8]| {
            // Extract VID from key format: {space_id}:vertex:{vid}
            let key_str = String::from_utf8_lossy(key);
            let parts: Vec<&str> = key_str.split(':').collect();
            if parts.len() < 3 {
                return false;
            }

            // Parse VID and check partition
            if let Ok(vid) = parts[2].parse::<i64>() {
                let computed_part = byoridb_common::hash::compute_partition(vid, partition_num);
                if computed_part != part_id {
                    return false;
                }
            } else {
                return false;
            }

            // Filter by tag if specified
            if !tag_filter.is_empty() {
                // Use codec to decode vertex (supports both Proto and JSON formats)
                if let Ok(vertex_data) = byoridb_codec::VertexCodec::decode_vertex(value) {
                    return vertex_data.tags.iter().any(|tag| tag.name == tag_filter);
                }
                return false;
            }

            true
        });

        match self
            .kvstore
            .scan_with_filter(&start_key, filter_fn, Some(limit + 1))
            .await
        {
            Ok(results) => {
                let has_more = results.len() > limit;
                let actual_results: Vec<_> = results.into_iter().take(limit).collect();

                let next_cursor = if has_more {
                    actual_results
                        .last()
                        .map(|(k, _)| {
                            // Create cursor for next page (key after last)
                            let mut next = k.clone();
                            next.push(0);
                            next
                        })
                        .unwrap_or_default()
                } else {
                    vec![]
                };

                let mut vertices = Vec::new();
                for (key, value) in actual_results {
                    // Use codec to decode vertex (supports both Proto and JSON formats)
                    if let Ok(vertex_data) = Codec::decode_vertex(&value) {
                        // Use VID from decoded data instead of parsing key
                        let vid = vertex_data.vid;

                        let mut tag_data_list = Vec::new();
                        for tag in &vertex_data.tags {
                            // Filter by tag name if specified
                            if !req.tag_name.is_empty() && tag.name != req.tag_name {
                                continue;
                            }

                            let mut properties: HashMap<String, Vec<u8>> = HashMap::new();
                            for (prop_key, prop_value) in &tag.properties {
                                if !req.prop_names.is_empty() && !req.prop_names.contains(prop_key)
                                {
                                    continue;
                                }
                                // Encode property value using bincode
                                if let Ok(bytes) = bincode::serialize(prop_value) {
                                    properties.insert(prop_key.clone(), bytes);
                                }
                            }

                            tag_data_list.push(TagData {
                                tag_name: tag.name.clone(),
                                properties,
                            });
                        }

                        vertices.push(VertexData {
                            vid,
                            tags: tag_data_list,
                        });
                    } else {
                        // Fallback: extract VID from key if decode fails
                        let key_str = String::from_utf8_lossy(&key);
                        let vid = key_str
                            .split(':')
                            .nth(2)
                            .and_then(|s| s.parse::<i64>().ok())
                            .unwrap_or(0);
                        warn!("Failed to decode vertex for vid={}", vid);
                    }
                }

                Ok(Response::new(ScanVerticesResponse {
                    code: ErrorCode::Succeeded as i32,
                    vertices,
                    next_cursor,
                    has_more,
                    error_message: String::new(),
                }))
            }
            Err(e) => {
                error!("Failed to scan vertices: {}", e);
                Ok(Response::new(ScanVerticesResponse {
                    code: ErrorCode::EInternalError as i32,
                    vertices: vec![],
                    next_cursor: vec![],
                    has_more: false,
                    error_message: format!("Failed to scan vertices: {}", e),
                }))
            }
        }
    }

    /// Scan edges in a partition
    async fn scan_edges(
        &self,
        request: Request<ScanEdgesRequest>,
    ) -> Result<Response<ScanEdgesResponse>, Status> {
        let req = request.into_inner();
        let space_id = req.space_id;
        let part_id = req.part_id;

        debug!(
            "ScanEdges: space_id={}, part_id={}, edge_type={}",
            space_id, part_id, req.edge_type
        );

        // Verify we own this partition
        if !self
            .partition_manager
            .owns_partition(space_id, part_id)
            .await
        {
            return Ok(Response::new(ScanEdgesResponse {
                code: ErrorCode::EPartitionNotFound as i32,
                edges: vec![],
                next_cursor: vec![],
                has_more: false,
                error_message: format!("Partition {} not owned by this node", part_id),
            }));
        }

        // Build scan prefix for edges
        let prefix = format!("{}:edge:", space_id);
        let limit = if req.limit > 0 {
            req.limit as usize
        } else {
            1000
        };

        let start_key = if req.cursor.is_empty() {
            prefix.clone().into_bytes()
        } else {
            req.cursor.clone()
        };

        // Create filter for partition and edge type
        let partition_num = self
            .partition_manager
            .get_partition_num(space_id)
            .await
            .unwrap_or(100);
        let edge_type_filter = req.edge_type.clone();

        let filter_fn: byoridb_kvstore::FilterFn = Box::new(move |key: &[u8], _value: &[u8]| {
            // Key format: {space_id}:edge:{src_vid}:{edge_type}:{ranking}
            let key_str = String::from_utf8_lossy(key);
            let parts: Vec<&str> = key_str.split(':').collect();
            if parts.len() < 4 {
                return false;
            }

            // Parse src_vid and check partition
            if let Ok(src_vid) = parts[2].parse::<i64>() {
                let computed_part = byoridb_common::hash::compute_partition(src_vid, partition_num);
                if computed_part != part_id {
                    return false;
                }
            } else {
                return false;
            }

            // Filter by edge type if specified
            if !edge_type_filter.is_empty() && parts.len() >= 4 && parts[3] != edge_type_filter {
                return false;
            }

            true
        });

        match self
            .kvstore
            .scan_with_filter(&start_key, filter_fn, Some(limit + 1))
            .await
        {
            Ok(results) => {
                let has_more = results.len() > limit;
                let actual_results: Vec<_> = results.into_iter().take(limit).collect();

                let next_cursor = if has_more {
                    actual_results
                        .last()
                        .map(|(k, _)| {
                            let mut next = k.clone();
                            next.push(0);
                            next
                        })
                        .unwrap_or_default()
                } else {
                    vec![]
                };

                let mut edges = Vec::new();
                for (key, value) in actual_results {
                    // Use codec to decode edge (supports both Proto and JSON formats)
                    if let Ok(edge_data) = Codec::decode_edge(&value) {
                        let mut properties: HashMap<String, Vec<u8>> = HashMap::new();
                        for (prop_key, prop_value) in &edge_data.properties {
                            if !req.prop_names.is_empty() && !req.prop_names.contains(prop_key) {
                                continue;
                            }
                            // Encode property value using bincode
                            if let Ok(bytes) = bincode::serialize(prop_value) {
                                properties.insert(prop_key.clone(), bytes);
                            }
                        }

                        edges.push(EdgeData {
                            key: Some(EdgeKey {
                                src_vid: edge_data.src_vid,
                                dst_vid: edge_data.dst_vid,
                                edge_type: edge_data.edge_type.clone(),
                                ranking: edge_data.ranking,
                            }),
                            properties,
                        });
                    } else {
                        // Fallback: log warning if decode fails
                        let key_str = String::from_utf8_lossy(&key);
                        warn!("Failed to decode edge for key={}", key_str);
                    }
                }

                Ok(Response::new(ScanEdgesResponse {
                    code: ErrorCode::Succeeded as i32,
                    edges,
                    next_cursor,
                    has_more,
                    error_message: String::new(),
                }))
            }
            Err(e) => {
                error!("Failed to scan edges: {}", e);
                Ok(Response::new(ScanEdgesResponse {
                    code: ErrorCode::EInternalError as i32,
                    edges: vec![],
                    next_cursor: vec![],
                    has_more: false,
                    error_message: format!("Failed to scan edges: {}", e),
                }))
            }
        }
    }

    /// Targeted neighbor fetch — read out-edges of each requested src_vid
    /// with an O(degree) prefix scan instead of a partition-wide scan.
    ///
    /// This replaces the `scan_edges` + post-filter pattern used by
    /// distributed `GO` when the source-VID set is small. The wire savings
    /// dominate: a partition with N edges and a query touching k source
    /// vertices each with d-out average degree goes from O(N) to O(k·d).
    async fn get_neighbors_by_source(
        &self,
        request: Request<GetNeighborsBySourceRequest>,
    ) -> Result<Response<GetNeighborsBySourceResponse>, Status> {
        let req = request.into_inner();
        let space_id = req.space_id;
        let part_id = req.part_id;

        debug!(
            "GetNeighborsBySource: space_id={}, part_id={}, src_count={}, edge_types={:?}",
            space_id,
            part_id,
            req.src_vids.len(),
            req.edge_types
        );

        if !self
            .partition_manager
            .owns_partition(space_id, part_id)
            .await
        {
            return Ok(Response::new(GetNeighborsBySourceResponse {
                code: ErrorCode::EPartitionNotFound as i32,
                edges: vec![],
                error_message: format!("Partition {} not owned by this node", part_id),
                sources_with_edges: 0,
            }));
        }

        let partition_num = self
            .partition_manager
            .get_partition_num(space_id)
            .await
            .unwrap_or(100);

        let limit_per_src = if req.limit_per_src > 0 {
            Some(req.limit_per_src as usize)
        } else {
            None
        };

        let mut all_edges: Vec<EdgeData> = Vec::new();
        let mut sources_with_edges: u32 = 0;
        let edge_types_owned: Vec<String> = req.edge_types.clone();

        for src_vid in &req.src_vids {
            // Guard against cross-partition src VIDs reaching this node by
            // accident. Silently skip rather than error to support a caller
            // that fans out the same list to multiple partitions.
            let computed_part = byoridb_common::hash::compute_partition(*src_vid, partition_num);
            if computed_part != part_id {
                continue;
            }

            let prefix = format!("{}:edge:{}:", space_id, src_vid);
            let edge_types_for_filter: std::collections::HashSet<String> =
                edge_types_owned.iter().cloned().collect();
            let filter_fn: byoridb_kvstore::FilterFn = Box::new(move |key: &[u8], _v: &[u8]| {
                if edge_types_for_filter.is_empty() {
                    return true;
                }
                let key_str = String::from_utf8_lossy(key);
                let parts: Vec<&str> = key_str.split(':').collect();
                if parts.len() < 5 {
                    return false;
                }
                edge_types_for_filter.contains(parts[3])
            });

            let results = match self
                .kvstore
                .scan_with_filter(prefix.as_bytes(), filter_fn, limit_per_src)
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    error!(
                        "GetNeighborsBySource scan failed for src={}: {}",
                        src_vid, e
                    );
                    continue;
                }
            };

            if !results.is_empty() {
                sources_with_edges += 1;
            }

            for (_, value) in results {
                let edge_data = match Codec::decode_edge(&value) {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                let mut properties: HashMap<String, Vec<u8>> = HashMap::new();
                for (prop_key, prop_value) in &edge_data.properties {
                    if !req.prop_names.is_empty() && !req.prop_names.contains(prop_key) {
                        continue;
                    }
                    if let Ok(bytes) = bincode::serialize(prop_value) {
                        properties.insert(prop_key.clone(), bytes);
                    }
                }

                all_edges.push(EdgeData {
                    key: Some(EdgeKey {
                        src_vid: edge_data.src_vid,
                        dst_vid: edge_data.dst_vid,
                        edge_type: edge_data.edge_type.clone(),
                        ranking: edge_data.ranking,
                    }),
                    properties,
                });
            }
        }

        Ok(Response::new(GetNeighborsBySourceResponse {
            code: ErrorCode::Succeeded as i32,
            edges: all_edges,
            error_message: String::new(),
            sources_with_edges,
        }))
    }

    // ===== Index Lookup RPCs =====

    /// Lookup vertices by tag index
    async fn lookup_tag_index(
        &self,
        request: Request<LookupTagIndexRequest>,
    ) -> Result<Response<LookupTagIndexResponse>, Status> {
        let req = request.into_inner();
        let space_id = req.space_id;
        let part_id = req.part_id;

        debug!(
            "LookupTagIndex: space_id={}, part_id={}, index_id={}, index_name={}",
            space_id, part_id, req.index_id, req.index_name
        );

        // Verify we own this partition
        if !self
            .partition_manager
            .owns_partition(space_id, part_id)
            .await
        {
            return Ok(Response::new(LookupTagIndexResponse {
                code: ErrorCode::EPartitionNotFound as i32,
                vids: vec![],
                next_cursor: vec![],
                has_more: false,
                error_message: format!("Partition {} not owned by this node", part_id),
            }));
        }

        // Get index definition (by ID or name)
        let index_def = if req.index_id > 0 {
            self.index_manager
                .get_index_by_id(space_id, req.index_id)
                .await
        } else if !req.index_name.is_empty() {
            self.index_manager
                .get_index(space_id, &req.index_name)
                .await
        } else {
            None
        };

        let index_def = match index_def {
            Some(def) if def.index_type == IndexType::Tag => def,
            Some(_) => {
                return Ok(Response::new(LookupTagIndexResponse {
                    code: ErrorCode::EInvalidRequest as i32,
                    vids: vec![],
                    next_cursor: vec![],
                    has_more: false,
                    error_message: "Index is not a tag index".to_string(),
                }));
            }
            None => {
                return Ok(Response::new(LookupTagIndexResponse {
                    code: ErrorCode::EInvalidRequest as i32,
                    vids: vec![],
                    next_cursor: vec![],
                    has_more: false,
                    error_message: format!(
                        "Index not found: id={}, name={}",
                        req.index_id, req.index_name
                    ),
                }));
            }
        };

        // Convert proto values to storage values
        let values: Vec<StorageIndexValue> = req
            .values
            .iter()
            .filter_map(Self::convert_proto_index_value)
            .collect();

        let limit = if req.limit > 0 {
            req.limit as usize
        } else {
            1000
        };

        // Perform index lookup
        match self
            .index_manager
            .lookup_tag(part_id, &index_def, &values, limit + 1)
            .await
        {
            Ok(vids) => {
                let has_more = vids.len() > limit;
                let result_vids: Vec<i64> = vids.into_iter().take(limit).collect();

                let next_cursor = if has_more && !result_vids.is_empty() {
                    // Use last VID as cursor
                    result_vids
                        .last()
                        .map(|v| v.to_le_bytes().to_vec())
                        .unwrap_or_default()
                } else {
                    vec![]
                };

                Ok(Response::new(LookupTagIndexResponse {
                    code: ErrorCode::Succeeded as i32,
                    vids: result_vids,
                    next_cursor,
                    has_more,
                    error_message: String::new(),
                }))
            }
            Err(e) => {
                error!("Failed to lookup tag index: {}", e);
                Ok(Response::new(LookupTagIndexResponse {
                    code: ErrorCode::EInternalError as i32,
                    vids: vec![],
                    next_cursor: vec![],
                    has_more: false,
                    error_message: format!("Index lookup failed: {}", e),
                }))
            }
        }
    }

    /// Lookup edges by edge index
    async fn lookup_edge_index(
        &self,
        request: Request<LookupEdgeIndexRequest>,
    ) -> Result<Response<LookupEdgeIndexResponse>, Status> {
        let req = request.into_inner();
        let space_id = req.space_id;
        let part_id = req.part_id;

        debug!(
            "LookupEdgeIndex: space_id={}, part_id={}, index_id={}, index_name={}",
            space_id, part_id, req.index_id, req.index_name
        );

        // Verify we own this partition
        if !self
            .partition_manager
            .owns_partition(space_id, part_id)
            .await
        {
            return Ok(Response::new(LookupEdgeIndexResponse {
                code: ErrorCode::EPartitionNotFound as i32,
                edges: vec![],
                next_cursor: vec![],
                has_more: false,
                error_message: format!("Partition {} not owned by this node", part_id),
            }));
        }

        // Get index definition
        let index_def = if req.index_id > 0 {
            self.index_manager
                .get_index_by_id(space_id, req.index_id)
                .await
        } else if !req.index_name.is_empty() {
            self.index_manager
                .get_index(space_id, &req.index_name)
                .await
        } else {
            None
        };

        let index_def = match index_def {
            Some(def) if def.index_type == IndexType::Edge => def,
            Some(_) => {
                return Ok(Response::new(LookupEdgeIndexResponse {
                    code: ErrorCode::EInvalidRequest as i32,
                    edges: vec![],
                    next_cursor: vec![],
                    has_more: false,
                    error_message: "Index is not an edge index".to_string(),
                }));
            }
            None => {
                return Ok(Response::new(LookupEdgeIndexResponse {
                    code: ErrorCode::EInvalidRequest as i32,
                    edges: vec![],
                    next_cursor: vec![],
                    has_more: false,
                    error_message: format!(
                        "Index not found: id={}, name={}",
                        req.index_id, req.index_name
                    ),
                }));
            }
        };

        // Convert proto values to storage values
        let values: Vec<StorageIndexValue> = req
            .values
            .iter()
            .filter_map(Self::convert_proto_index_value)
            .collect();

        let limit = if req.limit > 0 {
            req.limit as usize
        } else {
            1000
        };

        // Perform index lookup
        match self
            .index_manager
            .lookup_edge(part_id, &index_def, &values, limit + 1)
            .await
        {
            Ok(edges) => {
                let has_more = edges.len() > limit;
                let edge_type_name = index_def.schema_name.clone();
                let result_edges: Vec<EdgeKey> = edges
                    .into_iter()
                    .take(limit)
                    .map(|(src_vid, rank, dst_vid)| EdgeKey {
                        src_vid,
                        dst_vid,
                        edge_type: edge_type_name.clone(),
                        ranking: rank,
                    })
                    .collect();

                let next_cursor = if has_more && !result_edges.is_empty() {
                    // Use last edge key as cursor
                    result_edges
                        .last()
                        .map(|e| {
                            let mut cursor = Vec::new();
                            cursor.extend_from_slice(&e.src_vid.to_le_bytes());
                            cursor.extend_from_slice(&e.ranking.to_le_bytes());
                            cursor.extend_from_slice(&e.dst_vid.to_le_bytes());
                            cursor
                        })
                        .unwrap_or_default()
                } else {
                    vec![]
                };

                Ok(Response::new(LookupEdgeIndexResponse {
                    code: ErrorCode::Succeeded as i32,
                    edges: result_edges,
                    next_cursor,
                    has_more,
                    error_message: String::new(),
                }))
            }
            Err(e) => {
                error!("Failed to lookup edge index: {}", e);
                Ok(Response::new(LookupEdgeIndexResponse {
                    code: ErrorCode::EInternalError as i32,
                    edges: vec![],
                    next_cursor: vec![],
                    has_more: false,
                    error_message: format!("Index lookup failed: {}", e),
                }))
            }
        }
    }

    /// Check if keys exist using Bloom filter
    async fn check_bloom_filter(
        &self,
        request: Request<CheckBloomFilterRequest>,
    ) -> Result<Response<CheckBloomFilterResponse>, Status> {
        let req = request.into_inner();
        let space_id = req.space_id;
        let part_id = req.part_id;

        debug!(
            "CheckBloomFilter: space_id={}, part_id={}, filter_type={:?}, keys_count={}",
            space_id,
            part_id,
            req.filter_type,
            req.keys.len()
        );

        // Verify we own this partition
        if !self
            .partition_manager
            .owns_partition(space_id, part_id)
            .await
        {
            return Ok(Response::new(CheckBloomFilterResponse {
                code: ErrorCode::EPartitionNotFound as i32,
                may_exist: vec![],
                error_message: format!("Partition {} not owned by this node", part_id),
            }));
        }

        // For now, we'll do actual existence checks
        // A real Bloom filter implementation would be more efficient
        let filter_type =
            BloomFilterType::try_from(req.filter_type).unwrap_or(BloomFilterType::BfVertex);

        let mut results = Vec::with_capacity(req.keys.len());

        match filter_type {
            BloomFilterType::BfVertex => {
                // Check vertex existence
                let keys: Vec<Vec<u8>> = req
                    .keys
                    .iter()
                    .map(|vid| format!("{}:vertex:{}", space_id, vid).into_bytes())
                    .collect();

                match self.kvstore.batch_get(&keys).await {
                    Ok(exists_results) => {
                        for exists in exists_results {
                            results.push(exists.is_some());
                        }
                    }
                    Err(e) => {
                        error!("Failed to check vertex existence: {}", e);
                        return Ok(Response::new(CheckBloomFilterResponse {
                            code: ErrorCode::EInternalError as i32,
                            may_exist: vec![],
                            error_message: format!("Bloom filter check failed: {}", e),
                        }));
                    }
                }
            }
            BloomFilterType::BfEdge => {
                // For edges, we need src_vid, but we only have keys as i64
                // Return true for all (conservative - might exist)
                results = vec![true; req.keys.len()];
            }
            BloomFilterType::BfTagIndex | BloomFilterType::BfEdgeIndex => {
                // Index existence check - conservative approach
                results = vec![true; req.keys.len()];
            }
        }

        Ok(Response::new(CheckBloomFilterResponse {
            code: ErrorCode::Succeeded as i32,
            may_exist: results,
            error_message: String::new(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::partition::{PartitionInfo, PartitionStatus};
    use byoridb_codec::{EdgeData as CodecEdgeData, VertexCodec};
    use byoridb_common::Value;
    use byoridb_kvstore::store::MemoryKVStore;

    /// Inserts a proto-encoded edge `{src}->{dst}` under
    /// `{space_id}:edge:{src}:{edge_type}:{ranking}`.
    async fn put_edge(
        kv: &Arc<MemoryKVStore>,
        space_id: u32,
        src: i64,
        dst: i64,
        edge_type: &str,
        ranking: i64,
    ) {
        let edge = CodecEdgeData {
            src_vid: src,
            dst_vid: dst,
            edge_type: edge_type.to_string(),
            ranking,
            properties: std::collections::HashMap::from([("since".to_string(), Value::Int(2024))]),
        };
        let bytes = VertexCodec::encode_edge(&edge).unwrap();
        let key = format!(
            "{}:edge:{}:{}:{}:{}",
            space_id, src, edge_type, dst, ranking
        );
        kv.put(key.as_bytes(), &bytes).await.unwrap();
    }

    #[tokio::test]
    async fn get_neighbors_by_source_returns_only_listed_src_vids() {
        let partition_num: u32 = 1;
        let memory = Arc::new(MemoryKVStore::new());
        let kvstore: Arc<dyn KVStore> = memory.clone();
        let pm = Arc::new(PartitionManager::new("127.0.0.1".to_string(), 0));
        pm.register_space(1, partition_num).await;
        pm.add_partition(PartitionInfo {
            space_id: 1,
            part_id: 1,
            status: PartitionStatus::Leader,
            leader: Some(("127.0.0.1".to_string(), 0)),
            peers: vec![],
        })
        .await;
        let service = StorageRpcService::new(pm, kvstore);

        put_edge(&memory, 1, 100, 200, "follow", 0).await;
        put_edge(&memory, 1, 100, 201, "follow", 1).await;
        put_edge(&memory, 1, 100, 202, "blocked", 0).await;
        // unrelated source — must NOT be returned
        put_edge(&memory, 1, 999, 300, "follow", 0).await;

        let request = tonic::Request::new(GetNeighborsBySourceRequest {
            space_id: 1,
            part_id: 1,
            src_vids: vec![100],
            edge_types: vec!["follow".to_string()],
            limit_per_src: 0,
            prop_names: vec![],
        });

        let response = service.get_neighbors_by_source(request).await.unwrap();
        let inner = response.into_inner();

        assert_eq!(inner.code, 0);
        assert_eq!(inner.sources_with_edges, 1);
        // 2 follow edges from src=100; the "blocked" one is filtered out
        // and the src=999 edge is not in the requested src_vids list.
        assert_eq!(inner.edges.len(), 2);
        for edge in &inner.edges {
            let key = edge.key.as_ref().unwrap();
            assert_eq!(key.src_vid, 100);
            assert_eq!(key.edge_type, "follow");
        }
    }

    #[tokio::test]
    async fn get_neighbors_by_source_rejects_unowned_partition() {
        let memory = Arc::new(MemoryKVStore::new());
        let kvstore: Arc<dyn KVStore> = memory.clone();
        let pm = Arc::new(PartitionManager::new("127.0.0.1".to_string(), 0));
        // Register space but do NOT add the partition — node doesn't own it.
        pm.register_space(1, 1).await;
        let service = StorageRpcService::new(pm, kvstore);

        let request = tonic::Request::new(GetNeighborsBySourceRequest {
            space_id: 1,
            part_id: 1,
            src_vids: vec![100],
            edge_types: vec![],
            limit_per_src: 0,
            prop_names: vec![],
        });
        let response = service.get_neighbors_by_source(request).await.unwrap();
        let inner = response.into_inner();
        assert_eq!(inner.code, ErrorCode::EPartitionNotFound as i32);
    }
}

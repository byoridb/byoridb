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

fn decode_query_vertex(vid: i64, data: &[u8]) -> Result<byoridb_codec::VertexData, Status> {
    let invalid = || Status::data_loss("Invalid vertex metadata");
    let proto = Codec::is_proto_format(data);
    if !proto {
        let json: serde_json::Value = serde_json::from_slice(data).map_err(|_| invalid())?;
        if json
            .get("vid")
            .is_some_and(|value| value.as_i64() != Some(vid))
        {
            return Err(invalid());
        }
        let tags = json
            .get("tags")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(invalid)?;
        if tags.iter().any(|tag| {
            tag.get("name")
                .and_then(serde_json::Value::as_str)
                .is_none()
                || tag.get("props").is_some_and(|props| !props.is_object())
        }) {
            return Err(invalid());
        }
    }
    let mut vertex = Codec::decode_vertex(data).map_err(|_| invalid())?;
    if proto && vertex.vid != vid {
        return Err(invalid());
    }
    vertex.vid = vid;
    Ok(vertex)
}

fn decode_query_edge(key: &EdgeKey, data: &[u8]) -> Result<byoridb_codec::EdgeData, Status> {
    let invalid = || Status::data_loss("Invalid edge metadata");
    if !Codec::is_proto_format(data) {
        let json: serde_json::Value = serde_json::from_slice(data).map_err(|_| invalid())?;
        if json
            .get("src")
            .and_then(serde_json::Value::as_i64)
            .is_none()
            || json
                .get("dst")
                .and_then(serde_json::Value::as_i64)
                .is_none()
            || json
                .get("ranking")
                .or_else(|| json.get("rank"))
                .is_some_and(|value| value.as_i64().is_none())
            || json
                .get("props")
                .or_else(|| json.get("properties"))
                .is_some_and(|value| !value.is_object())
        {
            return Err(invalid());
        }
    }
    let edge = Codec::decode_edge(data).map_err(|_| invalid())?;
    if edge.src_vid != key.src_vid
        || edge.dst_vid != key.dst_vid
        || edge.ranking != key.ranking
        || edge.edge_type != key.edge_type
        || edge.edge_type.is_empty()
    {
        return Err(invalid());
    }
    Ok(edge)
}

fn encode_query_properties(
    properties: &HashMap<String, byoridb_common::Value>,
    requested: &[String],
) -> Result<HashMap<String, Vec<u8>>, Status> {
    properties
        .iter()
        .filter(|(name, _)| requested.is_empty() || requested.contains(name))
        .map(|(name, value)| {
            bincode::serialize(value)
                .map(|bytes| (name.clone(), bytes))
                .map_err(|error| Status::internal(format!("Failed to serialize property: {error}")))
        })
        .collect()
}

#[derive(serde::Serialize, serde::Deserialize)]
struct VertexScanCursor {
    version: u8,
    space_id: u32,
    part_id: u32,
    tag_name: String,
    prop_names: Vec<String>,
    last_key: Vec<u8>,
}

fn scan_key_vid(key: &[u8], prefix: &[u8]) -> Result<i64, Status> {
    let suffix = key
        .strip_prefix(prefix)
        .ok_or_else(|| Status::data_loss("Invalid vertex key"))?;
    let text = std::str::from_utf8(suffix).map_err(|_| Status::data_loss("Invalid vertex key"))?;
    let vid = text
        .parse::<i64>()
        .map_err(|_| Status::data_loss("Invalid vertex key"))?;
    if vid.to_string() != text {
        return Err(Status::data_loss("Invalid vertex key"));
    }
    Ok(vid)
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

    async fn ensure_query_partition(&self, space_id: u32, part_id: u32) -> Result<(), Status> {
        match self.partition_manager.get_status(space_id, part_id).await {
            LocalPartitionStatus::Leader | LocalPartitionStatus::Follower => Ok(()),
            _ => Err(Status::unavailable(format!(
                "Partition {part_id} in space {space_id} is not ready for queries"
            ))),
        }
    }

    async fn ensure_fetch_partitions(&self, space_id: u32, vids: &[i64]) -> Result<(), Status> {
        let partition_num = self
            .partition_manager
            .get_partition_num(space_id)
            .await
            .filter(|count| *count > 0)
            .ok_or_else(|| Status::failed_precondition("Invalid partition configuration"))?;
        let partitions: std::collections::HashSet<_> = vids
            .iter()
            .map(|vid| byoridb_common::hash::compute_partition(*vid, partition_num))
            .collect();
        for part_id in partitions {
            self.ensure_query_partition(space_id, part_id).await?;
        }
        Ok(())
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

        self.ensure_fetch_partitions(space_id, &req.vids).await?;

        let mut vertices = Vec::new();

        // Build keys for batch lookup
        let keys: Vec<Vec<u8>> = req
            .vids
            .iter()
            .map(|vid| format!("{}:vertex:{}", space_id, vid).into_bytes())
            .collect();

        let results = self
            .kvstore
            .batch_get(&keys)
            .await
            .map_err(|error| Status::internal(format!("Failed to fetch vertices: {error}")))?;
        if results.len() != req.vids.len() {
            return Err(Status::data_loss("Incomplete batch get response"));
        }
        for (vid, data) in req.vids.iter().zip(results) {
            let Some(data) = data else { continue };
            let vertex = decode_query_vertex(*vid, &data)?;
            let mut tags = Vec::new();
            for tag in vertex.tags {
                if !req.tag_names.is_empty() && !req.tag_names.contains(&tag.name) {
                    continue;
                }
                tags.push(TagData {
                    tag_name: tag.name,
                    properties: encode_query_properties(&tag.properties, &req.prop_names)?,
                });
            }
            vertices.push(VertexData { vid: *vid, tags });
        }
        Ok(Response::new(BatchGetVerticesResponse {
            code: ErrorCode::Succeeded as i32,
            vertices,
            error_message: String::new(),
        }))
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

        self.ensure_fetch_partitions(
            space_id,
            &req.edge_keys
                .iter()
                .map(|key| key.src_vid)
                .collect::<Vec<_>>(),
        )
        .await?;

        let mut edges = Vec::new();

        // Build keys for batch lookup
        let keys: Vec<Vec<u8>> = req
            .edge_keys
            .iter()
            .map(|ek| {
                format!(
                    "{}:edge:{}:{}:{}:{}",
                    space_id, ek.src_vid, ek.edge_type, ek.dst_vid, ek.ranking
                )
                .into_bytes()
            })
            .collect();

        let results = self
            .kvstore
            .batch_get(&keys)
            .await
            .map_err(|error| Status::internal(format!("Failed to fetch edges: {error}")))?;
        if results.len() != req.edge_keys.len() {
            return Err(Status::data_loss("Incomplete batch get response"));
        }
        for (key, data) in req.edge_keys.iter().zip(results) {
            let Some(data) = data else { continue };
            let edge = decode_query_edge(key, &data)?;
            edges.push(EdgeData {
                key: Some(key.clone()),
                properties: encode_query_properties(&edge.properties, &req.prop_names)?,
            });
        }
        Ok(Response::new(BatchGetEdgesResponse {
            code: ErrorCode::Succeeded as i32,
            edges,
            error_message: String::new(),
        }))
    }

    /// Scan vertices in a partition
    async fn scan_vertices(
        &self,
        request: Request<ScanVerticesRequest>,
    ) -> Result<Response<ScanVerticesResponse>, Status> {
        let req = request.into_inner();
        let space_id = req.space_id;
        let part_id = req.part_id;
        self.ensure_query_partition(space_id, part_id).await?;

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

        let partition_num = self
            .partition_manager
            .get_partition_num(space_id)
            .await
            .filter(|count| *count > 0 && part_id > 0 && part_id <= *count)
            .ok_or_else(|| Status::failed_precondition("Invalid partition configuration"))?;
        let prefix = format!("{space_id}:vertex:").into_bytes();
        let mut end = prefix.clone();
        *end.last_mut().unwrap() += 1;
        let mut start = if req.cursor.is_empty() {
            prefix.clone()
        } else {
            let cursor: VertexScanCursor = serde_json::from_slice(&req.cursor)
                .map_err(|_| Status::invalid_argument("Invalid vertex scan cursor"))?;
            if cursor.version != 1
                || cursor.space_id != space_id
                || cursor.part_id != part_id
                || cursor.tag_name != req.tag_name
                || cursor.prop_names != req.prop_names
                || byoridb_common::hash::compute_partition(
                    scan_key_vid(&cursor.last_key, &prefix)?,
                    partition_num,
                ) != part_id
            {
                return Err(Status::invalid_argument(
                    "Vertex scan cursor does not match this query",
                ));
            }
            let mut start = cursor.last_key;
            start.push(0);
            start
        };
        let limit = if req.limit == 0 {
            1000
        } else {
            req.limit as usize
        };
        let mut vertices = Vec::new();
        let mut last_key = Vec::new();
        let mut has_more = false;
        'scan: loop {
            let entries = self
                .kvstore
                .scan_range(&start, &end, Some(256))
                .await
                .map_err(|error| Status::internal(format!("Failed to scan vertices: {error}")))?;
            let count = entries.len();
            for (key, value) in entries {
                start = key.clone();
                start.push(0);
                let vid = scan_key_vid(&key, &prefix)?;
                if byoridb_common::hash::compute_partition(vid, partition_num) != part_id {
                    continue;
                }
                let vertex = decode_query_vertex(vid, &value)?;
                if !req.tag_name.is_empty()
                    && !vertex.tags.iter().any(|tag| tag.name == req.tag_name)
                {
                    continue;
                }
                if vertices.len() == limit {
                    has_more = true;
                    break 'scan;
                }
                let mut tags = Vec::new();
                for tag in vertex.tags {
                    if !req.tag_name.is_empty() && tag.name != req.tag_name {
                        continue;
                    }
                    tags.push(TagData {
                        tag_name: tag.name,
                        properties: encode_query_properties(&tag.properties, &req.prop_names)?,
                    });
                }
                vertices.push(VertexData { vid, tags });
                last_key = key;
            }
            if count < 256 {
                break;
            }
        }
        let next_cursor = if has_more {
            serde_json::to_vec(&VertexScanCursor {
                version: 1,
                space_id,
                part_id,
                tag_name: req.tag_name,
                prop_names: req.prop_names,
                last_key,
            })
            .map_err(|error| Status::internal(error.to_string()))?
        } else {
            vec![]
        };
        Ok(Response::new(ScanVerticesResponse {
            code: ErrorCode::Succeeded as i32,
            vertices,
            next_cursor,
            has_more,
            error_message: String::new(),
        }))
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
        self.ensure_query_partition(space_id, part_id).await?;

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
            .filter(|count| *count > 0)
            .ok_or_else(|| Status::failed_precondition("Invalid partition configuration"))?;

        let limit_per_src = if req.limit_per_src > 0 {
            Some(req.limit_per_src as usize)
        } else {
            None
        };

        let mut all_edges: Vec<EdgeData> = Vec::new();
        let mut sources_with_edges: u32 = 0;
        let edge_types_owned: Vec<String> = req.edge_types.clone();

        for src_vid in &req.src_vids {
            let computed_part = byoridb_common::hash::compute_partition(*src_vid, partition_num);
            if computed_part != part_id {
                return Err(Status::invalid_argument(
                    "Source VID belongs to another partition",
                ));
            }

            let prefix = format!("{}:edge:{}:", space_id, src_vid);
            let results = self
                .kvstore
                .scan_prefix(prefix.as_bytes())
                .await
                .map_err(|error| Status::internal(format!("Neighbor scan failed: {error}")))?;
            let mut count = 0;
            for (key, value) in results {
                let suffix = key
                    .strip_prefix(prefix.as_bytes())
                    .and_then(|suffix| std::str::from_utf8(suffix).ok())
                    .ok_or_else(|| Status::data_loss("Invalid neighbor edge key"))?;
                let parts: Vec<_> = suffix.split(':').collect();
                if parts.len() != 3 || parts[0].is_empty() {
                    return Err(Status::data_loss("Invalid neighbor edge key"));
                }
                let dst_vid = parts[1]
                    .parse::<i64>()
                    .map_err(|_| Status::data_loss("Invalid neighbor destination"))?;
                let ranking = parts[2]
                    .parse::<i64>()
                    .map_err(|_| Status::data_loss("Invalid neighbor rank"))?;
                if dst_vid.to_string() != parts[1] || ranking.to_string() != parts[2] {
                    return Err(Status::data_loss("Noncanonical neighbor edge key"));
                }
                let edge_key = EdgeKey {
                    src_vid: *src_vid,
                    dst_vid,
                    ranking,
                    edge_type: parts[0].to_owned(),
                };
                let edge_data = decode_query_edge(&edge_key, &value)?;
                if !edge_types_owned.is_empty() && !edge_types_owned.contains(&edge_data.edge_type)
                {
                    continue;
                }
                let properties = encode_query_properties(&edge_data.properties, &req.prop_names)?;

                all_edges.push(EdgeData {
                    key: Some(EdgeKey {
                        src_vid: edge_data.src_vid,
                        dst_vid: edge_data.dst_vid,
                        edge_type: edge_data.edge_type.clone(),
                        ranking: edge_data.ranking,
                    }),
                    properties,
                });
                count += 1;
                if limit_per_src.is_some_and(|limit| count >= limit) {
                    break;
                }
            }
            if count > 0 {
                sources_with_edges += 1;
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
        self.ensure_query_partition(space_id, part_id).await?;

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
            Some(def)
                if def.index_type == IndexType::Tag
                    && def.space_id == space_id
                    && (req.index_name.is_empty() || req.index_name == def.index_name) =>
            {
                def
            }
            Some(_) => {
                return Ok(Response::new(LookupTagIndexResponse {
                    code: ErrorCode::EInvalidRequest as i32,
                    vids: vec![],
                    next_cursor: vec![],
                    has_more: false,
                    error_message: "Tag index identity or type does not match the request"
                        .to_string(),
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
            .lookup_tag_page(part_id, &index_def, &values, limit, &req.cursor)
            .await
        {
            Ok((result_vids, next_cursor, has_more)) => Ok(Response::new(LookupTagIndexResponse {
                code: ErrorCode::Succeeded as i32,
                vids: result_vids,
                next_cursor,
                has_more,
                error_message: String::new(),
            })),
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
        self.ensure_query_partition(space_id, part_id).await?;

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
            Some(def)
                if def.index_type == IndexType::Edge
                    && def.space_id == space_id
                    && (req.index_name.is_empty() || req.index_name == def.index_name) =>
            {
                def
            }
            Some(_) => {
                return Ok(Response::new(LookupEdgeIndexResponse {
                    code: ErrorCode::EInvalidRequest as i32,
                    edges: vec![],
                    next_cursor: vec![],
                    has_more: false,
                    error_message: "Edge index identity or type does not match the request"
                        .to_string(),
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
            .lookup_edge_page(part_id, &index_def, &values, limit, &req.cursor)
            .await
        {
            Ok((edges, next_cursor, has_more)) => {
                let result_edges = edges
                    .into_iter()
                    .map(|(src_vid, ranking, dst_vid)| EdgeKey {
                        src_vid,
                        dst_vid,
                        ranking,
                        edge_type: index_def.schema_name.clone(),
                    })
                    .collect();
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
        // Locally present data must not bypass the partition readiness check.
        put_edge(&memory, 1, 100, 200, "follow", 0).await;
        let service = StorageRpcService::new(pm, kvstore);

        let request = tonic::Request::new(GetNeighborsBySourceRequest {
            space_id: 1,
            part_id: 1,
            src_vids: vec![100],
            edge_types: vec![],
            limit_per_src: 0,
            prop_names: vec![],
        });
        let error = service.get_neighbors_by_source(request).await.unwrap_err();
        assert_eq!(error.code(), tonic::Code::Unavailable);
    }
}

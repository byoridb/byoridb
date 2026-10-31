// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Distributed Query Executor
//!
//! This module provides distributed query execution across multiple Storage nodes.
//! It handles:
//! - VID to partition mapping
//! - Partition to host lookup via MetaClient
//! - Parallel RPC execution across storage nodes
//! - Result aggregation

use crate::storage_client::{StorageClientError, StorageQueryClient};
use byoridb_meta::{schema::PartAllocation, MetaClient};
use byoridb_storage::proto::storage::{BloomFilterType, EdgeData, EdgeKey, IndexValue, VertexData};
use futures::future::join_all;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use thiserror::Error;
use tracing::{debug, error, info, warn};

/// Error type for distributed query execution
#[derive(Error, Debug)]
pub enum DistributedQueryError {
    #[error("Meta client error: {0}")]
    MetaError(String),

    #[error("Storage client error: {0}")]
    StorageError(#[from] StorageClientError),

    #[error("No partition hosts found for partition {part_id}")]
    NoPartitionHosts { part_id: u32 },

    #[error("Partition lookup failed: {0}")]
    PartitionLookupFailed(String),

    #[error("All replicas failed for partition {part_id}")]
    AllReplicasFailed { part_id: u32 },

    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("LOOKUP candidate scan exceeded max_scan_limit ({cap}) in partition {part_id}; query result is incomplete")]
    ScanBudgetExceeded { part_id: u32, cap: usize },
}

pub type Result<T> = std::result::Result<T, DistributedQueryError>;

/// Configuration for distributed query execution
#[derive(Debug, Clone)]
pub struct DistributedQueryConfig {
    /// Maximum parallel requests per query
    pub max_parallel_requests: usize,
    /// Retry count for failed requests
    pub retry_count: usize,
    /// Whether to read from followers (for load balancing)
    pub read_from_followers: bool,
}

impl Default for DistributedQueryConfig {
    fn default() -> Self {
        Self {
            max_parallel_requests: 100,
            retry_count: 2,
            read_from_followers: false,
        }
    }
}

/// Distributed Query Executor
///
/// Handles distributed query execution by:
/// 1. Computing partition for each VID (hash-based)
/// 2. Looking up partition hosts from Meta service
/// 3. Executing parallel RPCs to storage nodes
/// 4. Aggregating and returning results
pub struct DistributedQueryExecutor {
    storage_client: Arc<StorageQueryClient>,
    meta_client: Arc<MetaClient>,
    #[allow(dead_code)]
    config: DistributedQueryConfig,
}

fn invalid_response(message: &str) -> DistributedQueryError {
    StorageClientError::InvalidResponse(message.to_string()).into()
}

fn validate_allocations(
    space_id: u32,
    partition_num: u32,
    mut allocs: Vec<PartAllocation>,
) -> Result<Vec<PartAllocation>> {
    if space_id == 0 || partition_num == 0 || allocs.len() != partition_num as usize {
        return Err(DistributedQueryError::InvalidConfig(format!(
            "expected {partition_num} partition allocations for space {space_id}, got {}",
            allocs.len()
        )));
    }
    allocs.sort_by_key(|alloc| alloc.part_id);
    for (index, alloc) in allocs.iter().enumerate() {
        if alloc.space_id != space_id || alloc.part_id != index as u32 + 1 {
            return Err(DistributedQueryError::InvalidConfig(
                "partition allocations must uniquely cover the requested space and partitions 1..N"
                    .to_string(),
            ));
        }
        if alloc.hosts.is_empty() {
            return Err(DistributedQueryError::NoPartitionHosts {
                part_id: alloc.part_id,
            });
        }
        if alloc
            .hosts
            .iter()
            .any(|(host, port)| host.trim().is_empty() || *port == 0 || *port > u16::MAX as u32)
        {
            return Err(DistributedQueryError::InvalidConfig(
                "partition allocation contains an invalid host or port".to_string(),
            ));
        }
    }
    Ok(allocs)
}

fn lookup_page_limit(cap: usize, already_read: usize) -> u32 {
    if cap == 0 {
        1024
    } else {
        cap.saturating_sub(already_read).min(1024) as u32
    }
}

fn validate_lookup_page(
    len: usize,
    requested: u32,
    has_more: bool,
    cursor: &[u8],
    seen: &mut HashSet<Vec<u8>>,
) -> Result<()> {
    if len > requested as usize
        || (has_more && (len == 0 || cursor.is_empty()))
        || (!has_more && !cursor.is_empty())
    {
        return Err(invalid_response("invalid index pagination response"));
    }
    if has_more && !seen.insert(cursor.to_vec()) {
        return Err(invalid_response("index pagination did not advance"));
    }
    Ok(())
}

impl DistributedQueryExecutor {
    /// Create a new distributed query executor
    pub fn new(storage_client: Arc<StorageQueryClient>, meta_client: Arc<MetaClient>) -> Self {
        Self::with_config(
            storage_client,
            meta_client,
            DistributedQueryConfig::default(),
        )
    }

    /// Create a new distributed query executor with custom config
    pub fn with_config(
        storage_client: Arc<StorageQueryClient>,
        meta_client: Arc<MetaClient>,
        config: DistributedQueryConfig,
    ) -> Self {
        Self {
            storage_client,
            meta_client,
            config,
        }
    }

    /// Group VIDs by partition
    ///
    /// Uses consistent hashing: partition_id = hash(vid) % partition_num + 1
    pub fn group_vids_by_partition(vids: &[i64], partition_num: u32) -> HashMap<u32, Vec<i64>> {
        let mut groups: HashMap<u32, Vec<i64>> = HashMap::new();

        for vid in vids {
            let part_id = byoridb_common::hash::compute_partition(*vid, partition_num);
            groups.entry(part_id).or_default().push(*vid);
        }

        debug!(
            "Grouped {} VIDs into {} partitions",
            vids.len(),
            groups.len()
        );

        groups
    }

    /// Group EdgeKeys by source partition
    pub fn group_edges_by_partition(
        edge_keys: &[EdgeKey],
        partition_num: u32,
    ) -> HashMap<u32, Vec<EdgeKey>> {
        let mut groups: HashMap<u32, Vec<EdgeKey>> = HashMap::new();

        for edge_key in edge_keys {
            let part_id = byoridb_common::hash::compute_partition(edge_key.src_vid, partition_num);
            groups.entry(part_id).or_default().push(edge_key.clone());
        }

        debug!(
            "Grouped {} edges into {} partitions",
            edge_keys.len(),
            groups.len()
        );

        groups
    }

    /// Get partition hosts from Meta service
    async fn get_partition_host(&self, space_id: u32, part_id: u32) -> Result<(String, u32)> {
        match self.meta_client.get_parts_alloc(space_id).await {
            Ok(allocs) => {
                for alloc in allocs {
                    if alloc.part_id == part_id {
                        if let Some((host, port)) = alloc.hosts.first() {
                            return Ok((host.clone(), *port));
                        }
                    }
                }
                Err(DistributedQueryError::NoPartitionHosts { part_id })
            }
            Err(e) => Err(DistributedQueryError::MetaError(e.to_string())),
        }
    }

    /// Execute distributed FETCH (batch get vertices)
    ///
    /// 1. Groups VIDs by partition
    /// 2. Looks up partition hosts from Meta
    /// 3. Executes parallel RPCs to each storage node
    /// 4. Merges and returns results
    pub async fn execute_fetch(
        &self,
        space_id: u32,
        partition_num: u32,
        vids: Vec<i64>,
        tag_names: Vec<String>,
        prop_names: Vec<String>,
    ) -> Result<Vec<VertexData>> {
        let allocs = self.partition_hosts(space_id, partition_num).await?;
        if vids.is_empty() {
            return Ok(vec![]);
        }

        info!(
            "Executing distributed FETCH: space_id={}, vids_count={}, partitions={}",
            space_id,
            vids.len(),
            partition_num
        );

        // Group VIDs by partition
        let vid_groups = Self::group_vids_by_partition(&vids, partition_num);

        // Create tasks for each partition
        let mut tasks = Vec::new();

        for (part_id, part_vids) in vid_groups {
            let storage_client = self.storage_client.clone();
            let tag_names = tag_names.clone();
            let prop_names = prop_names.clone();

            // Get partition host
            let (host, port) = allocs[(part_id - 1) as usize].hosts[0].clone();

            debug!(
                "Partition {} -> {}:{} ({} VIDs)",
                part_id,
                host,
                port,
                part_vids.len()
            );

            let task = async move {
                let requested: HashSet<_> = part_vids.iter().copied().collect();
                let response = storage_client
                    .batch_get_vertices(&host, port, space_id, part_vids, tag_names, prop_names)
                    .await?;
                if response
                    .vertices
                    .iter()
                    .any(|vertex| !requested.contains(&vertex.vid))
                {
                    return Err(StorageClientError::InvalidResponse(
                        "fetch returned an unrequested VID".to_string(),
                    ));
                }
                Ok(response)
            };

            tasks.push(task);
        }

        // Execute all tasks in parallel
        let results = join_all(tasks).await;

        // Aggregate results
        let mut all_vertices = Vec::new();
        let mut errors = Vec::new();

        for result in results {
            match result {
                Ok(response) => {
                    all_vertices.extend(response.vertices);
                }
                Err(e) => {
                    warn!("Partition fetch failed: {}", e);
                    errors.push(e);
                }
            }
        }

        // Any failed partition makes the aggregate incomplete. Never return
        // successfully fetched vertices as though they were the whole result.
        if let Some(error) = errors.pop() {
            return Err(error.into());
        }

        info!(
            "Distributed FETCH completed: {} vertices returned",
            all_vertices.len()
        );

        Ok(all_vertices)
    }

    /// Execute distributed edge fetch
    pub async fn execute_fetch_edges(
        &self,
        space_id: u32,
        partition_num: u32,
        edge_keys: Vec<EdgeKey>,
        prop_names: Vec<String>,
    ) -> Result<Vec<EdgeData>> {
        let allocs = self.partition_hosts(space_id, partition_num).await?;
        if edge_keys.is_empty() {
            return Ok(vec![]);
        }

        info!(
            "Executing distributed edge FETCH: space_id={}, edge_keys_count={}",
            space_id,
            edge_keys.len()
        );

        // Group edges by partition
        let edge_groups = Self::group_edges_by_partition(&edge_keys, partition_num);

        // Create tasks for each partition
        let mut tasks = Vec::new();

        for (part_id, part_edges) in edge_groups {
            let storage_client = self.storage_client.clone();
            let prop_names = prop_names.clone();

            // Get partition host
            let (host, port) = allocs[(part_id - 1) as usize].hosts[0].clone();

            debug!(
                "Partition {} -> {}:{} ({} edges)",
                part_id,
                host,
                port,
                part_edges.len()
            );

            let task = async move {
                let requested: HashSet<_> = part_edges
                    .iter()
                    .map(|key| (key.src_vid, key.dst_vid, key.ranking, key.edge_type.clone()))
                    .collect();
                let response = storage_client
                    .batch_get_edges(&host, port, space_id, part_edges, prop_names)
                    .await?;
                if response.edges.iter().any(|edge| {
                    edge.key.as_ref().is_none_or(|key| {
                        !requested.contains(&(
                            key.src_vid,
                            key.dst_vid,
                            key.ranking,
                            key.edge_type.clone(),
                        ))
                    })
                }) {
                    return Err(StorageClientError::InvalidResponse(
                        "fetch returned an unrequested or missing edge identity".to_string(),
                    ));
                }
                Ok(response)
            };

            tasks.push(task);
        }

        // Execute all tasks in parallel
        let results = join_all(tasks).await;

        // Aggregate results
        let mut all_edges = Vec::new();
        let mut errors = Vec::new();

        for result in results {
            match result {
                Ok(response) => {
                    all_edges.extend(response.edges);
                }
                Err(e) => {
                    warn!("Partition edge fetch failed: {}", e);
                    errors.push(e);
                }
            }
        }

        if let Some(error) = errors.pop() {
            return Err(error.into());
        }

        info!(
            "Distributed edge FETCH completed: {} edges returned",
            all_edges.len()
        );

        Ok(all_edges)
    }

    /// Execute distributed GO (graph traversal).
    ///
    /// Routes the source-VID list by partition and issues
    /// `GetNeighborsBySource` to each owning partition in parallel. This
    /// replaces the previous partition-wide `ScanEdges` + client-side
    /// filter pattern, which was O(partition_edges) regardless of how few
    /// source vertices the query actually touched.
    ///
    /// For each partition the server reads only the
    /// `{space_id}:edge:{src_vid}:` prefix for each VID assigned to it, so
    /// total work is O(Σ degree(src_vids)) — proportional to the answer
    /// size rather than the partition size.
    pub async fn execute_go(
        &self,
        space_id: u32,
        partition_num: u32,
        src_vids: Vec<i64>,
        edge_type: &str,
        prop_names: Vec<String>,
    ) -> Result<Vec<EdgeData>> {
        let allocs = self.partition_hosts(space_id, partition_num).await?;
        if src_vids.is_empty() {
            return Ok(vec![]);
        }

        info!(
            "Executing distributed GO: space_id={}, src_vids_count={}, edge_type={}",
            space_id,
            src_vids.len(),
            edge_type
        );

        let vid_groups = Self::group_vids_by_partition(&src_vids, partition_num);
        let mut tasks = Vec::new();

        for (part_id, part_vids) in vid_groups {
            let storage_client = self.storage_client.clone();
            let edge_types: Vec<String> = if edge_type.is_empty() {
                vec![]
            } else {
                vec![edge_type.to_string()]
            };
            let prop_names = prop_names.clone();
            let (host, port) = allocs[(part_id - 1) as usize].hosts[0].clone();

            let task = async move {
                storage_client
                    .get_neighbors_by_source(
                        &host, port, space_id, part_id, part_vids, edge_types,
                        0, // unlimited per source — caller applies any limit
                        prop_names,
                    )
                    .await
            };

            tasks.push(task);
        }

        let results = join_all(tasks).await;

        let mut all_edges = Vec::new();
        let mut errors = Vec::new();

        for result in results {
            match result {
                // Server already filtered by src_vid prefix, so we don't
                // need the post-filter HashSet pass anymore.
                Ok(response) => all_edges.extend(response.edges),
                Err(e) => {
                    warn!("GetNeighborsBySource failed: {}", e);
                    errors.push(e);
                }
            }
        }

        if let Some(error) = errors.pop() {
            return Err(error.into());
        }

        info!(
            "Distributed GO completed: {} edges returned",
            all_edges.len()
        );

        Ok(all_edges)
    }

    /// Execute distributed SCAN (full partition scan)
    ///
    /// Scans all partitions in parallel and returns all vertices matching the tag.
    pub async fn execute_scan(
        &self,
        space_id: u32,
        partition_num: u32,
        tag_name: &str,
        prop_names: Vec<String>,
        limit: u32,
    ) -> Result<Vec<VertexData>> {
        let allocs = self.partition_hosts(space_id, partition_num).await?;
        let tasks = allocs.into_iter().map(|alloc| {
            let prop_names = prop_names.clone();
            async move {
                let (host, port) = &alloc.hosts[0];
                let mut cursor = Vec::new();
                let mut seen_cursors = HashSet::new();
                let mut vertices = Vec::new();
                loop {
                    let page_limit = if limit == 0 {
                        1024
                    } else {
                        (limit as usize - vertices.len()).min(1024) as u32
                    };
                    let response = self
                        .storage_client
                        .scan_vertices(
                            host,
                            *port,
                            space_id,
                            alloc.part_id,
                            tag_name.to_string(),
                            prop_names.clone(),
                            cursor,
                            page_limit,
                        )
                        .await?;
                    validate_lookup_page(
                        response.vertices.len(),
                        page_limit,
                        response.has_more,
                        &response.next_cursor,
                        &mut seen_cursors,
                    )?;
                    vertices.extend(response.vertices);
                    if !response.has_more || (limit > 0 && vertices.len() >= limit as usize) {
                        return Ok::<_, DistributedQueryError>(vertices);
                    }
                    cursor = response.next_cursor;
                }
            }
        });
        let mut vertices = Vec::new();
        for result in join_all(tasks).await {
            vertices.extend(result?);
        }
        if limit > 0 {
            vertices.truncate(limit as usize);
        }
        Ok(vertices)
    }

    // ===== Index Lookup Methods =====

    /// All expected partitions must have valid, unambiguous ownership before
    /// any work is dispatched. A missing partition is not an empty partition.
    async fn partition_hosts(
        &self,
        space_id: u32,
        partition_num: u32,
    ) -> Result<Vec<PartAllocation>> {
        let allocs = self
            .meta_client
            .get_parts_alloc(space_id)
            .await
            .map_err(|e| DistributedQueryError::MetaError(e.to_string()))?;
        validate_allocations(space_id, partition_num, allocs)
    }

    /// Read every matching raw index entry in each partition. `max_scan_limit`
    /// is a per-partition safety budget (zero means unlimited), not SQL LIMIT.
    pub async fn execute_lookup_tag_index(
        &self,
        space_id: u32,
        partition_num: u32,
        index_id: u32,
        index_name: &str,
        values: Vec<IndexValue>,
        max_scan_limit: usize,
    ) -> Result<Vec<i64>> {
        let allocs = self.partition_hosts(space_id, partition_num).await?;
        let tasks = allocs.into_iter().map(|alloc| {
            let values = values.clone();
            async move {
                let (host, port) = &alloc.hosts[0];
                let mut cursor = Vec::new();
                let mut seen_cursors = HashSet::new();
                let mut vids = Vec::new();
                loop {
                    let page_limit = lookup_page_limit(max_scan_limit, vids.len());
                    let response = self
                        .storage_client
                        .lookup_tag_index(
                            host,
                            *port,
                            space_id,
                            alloc.part_id,
                            index_id,
                            index_name.to_string(),
                            values.clone(),
                            page_limit,
                            cursor.clone(),
                        )
                        .await?;
                    validate_lookup_page(
                        response.vids.len(),
                        page_limit,
                        response.has_more,
                        &response.next_cursor,
                        &mut seen_cursors,
                    )?;
                    for vid in &response.vids {
                        if byoridb_common::hash::compute_partition(*vid, partition_num)
                            != alloc.part_id
                        {
                            return Err(invalid_response("index VID belongs to another partition"));
                        }
                    }
                    vids.extend(response.vids);
                    if max_scan_limit > 0
                        && (vids.len() > max_scan_limit
                            || (vids.len() == max_scan_limit && response.has_more))
                    {
                        return Err(DistributedQueryError::ScanBudgetExceeded {
                            part_id: alloc.part_id,
                            cap: max_scan_limit,
                        });
                    }
                    if !response.has_more {
                        return Ok(vids);
                    }
                    cursor = response.next_cursor;
                }
            }
        });
        let mut vids = Vec::new();
        for result in join_all(tasks).await {
            vids.extend(result?);
        }
        Ok(vids)
    }

    /// Edge counterpart of `execute_lookup_tag_index`, with identical paging
    /// and per-partition completeness requirements.
    pub async fn execute_lookup_edge_index(
        &self,
        space_id: u32,
        partition_num: u32,
        index_id: u32,
        index_name: &str,
        values: Vec<IndexValue>,
        max_scan_limit: usize,
    ) -> Result<Vec<EdgeKey>> {
        let allocs = self.partition_hosts(space_id, partition_num).await?;
        let tasks = allocs.into_iter().map(|alloc| {
            let values = values.clone();
            async move {
                let (host, port) = &alloc.hosts[0];
                let mut cursor = Vec::new();
                let mut seen_cursors = HashSet::new();
                let mut edges = Vec::new();
                loop {
                    let page_limit = lookup_page_limit(max_scan_limit, edges.len());
                    let response = self
                        .storage_client
                        .lookup_edge_index(
                            host,
                            *port,
                            space_id,
                            alloc.part_id,
                            index_id,
                            index_name.to_string(),
                            values.clone(),
                            page_limit,
                            cursor.clone(),
                        )
                        .await?;
                    validate_lookup_page(
                        response.edges.len(),
                        page_limit,
                        response.has_more,
                        &response.next_cursor,
                        &mut seen_cursors,
                    )?;
                    for edge in &response.edges {
                        if byoridb_common::hash::compute_partition(edge.src_vid, partition_num)
                            != alloc.part_id
                        {
                            return Err(invalid_response(
                                "index edge belongs to another partition",
                            ));
                        }
                    }
                    edges.extend(response.edges);
                    if max_scan_limit > 0
                        && (edges.len() > max_scan_limit
                            || (edges.len() == max_scan_limit && response.has_more))
                    {
                        return Err(DistributedQueryError::ScanBudgetExceeded {
                            part_id: alloc.part_id,
                            cap: max_scan_limit,
                        });
                    }
                    if !response.has_more {
                        return Ok(edges);
                    }
                    cursor = response.next_cursor;
                }
            }
        });
        let mut edges = Vec::new();
        for result in join_all(tasks).await {
            edges.extend(result?);
        }
        Ok(edges)
    }

    /// Execute distributed Bloom filter check
    ///
    /// Checks all partitions for key existence and aggregates results.
    /// For each key, returns true if it MAY exist in any partition, false if it definitely doesn't exist.
    pub async fn check_bloom_filter(
        &self,
        space_id: u32,
        partition_num: u32,
        filter_type: BloomFilterType,
        keys: Vec<i64>,
    ) -> Result<Vec<bool>> {
        if keys.is_empty() {
            return Ok(vec![]);
        }

        info!(
            "Executing distributed Bloom filter check: space_id={}, keys_count={}",
            space_id,
            keys.len()
        );

        // Group keys by partition
        let key_groups = Self::group_vids_by_partition(&keys, partition_num);

        // Track which keys may exist (index by position in original array)
        let mut may_exist: Vec<bool> = vec![false; keys.len()];
        let key_to_index: HashMap<i64, Vec<usize>> =
            keys.iter()
                .enumerate()
                .fold(HashMap::new(), |mut acc, (idx, key)| {
                    acc.entry(*key).or_default().push(idx);
                    acc
                });

        // Create tasks for each partition
        let mut tasks = Vec::new();
        let mut partition_keys: Vec<(u32, Vec<i64>)> = Vec::new();

        for (part_id, part_keys) in key_groups {
            let storage_client = self.storage_client.clone();
            let keys_clone = part_keys.clone();
            partition_keys.push((part_id, part_keys));

            // Get partition host
            let (host, port) = self.get_partition_host(space_id, part_id).await?;

            let task = async move {
                storage_client
                    .check_bloom_filter(&host, port, space_id, part_id, filter_type, keys_clone)
                    .await
            };

            tasks.push(task);
        }

        // Execute all tasks in parallel
        let results = join_all(tasks).await;

        // Process results
        let mut errors = Vec::new();

        for (result, (_, part_keys)) in results.into_iter().zip(partition_keys.into_iter()) {
            match result {
                Ok(response) => {
                    // Map results back to original key positions
                    for (key, exists) in part_keys.iter().zip(response.may_exist.iter()) {
                        if *exists {
                            if let Some(indices) = key_to_index.get(key) {
                                for idx in indices {
                                    may_exist[*idx] = true;
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("Partition Bloom filter check failed: {}", e);
                    errors.push(e);
                }
            }
        }

        // If all partitions failed, return error
        if !errors.is_empty() && !may_exist.iter().any(|&v| v) {
            error!("All partition Bloom filter checks failed");
            // Return true for all keys when we can't check (conservative approach)
            return Ok(vec![true; keys.len()]);
        }

        info!(
            "Distributed Bloom filter check completed: {}/{} keys may exist",
            may_exist.iter().filter(|&&v| v).count(),
            keys.len()
        );

        Ok(may_exist)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_group_vids_by_partition() {
        let vids = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let partition_num = 3;

        let groups = DistributedQueryExecutor::group_vids_by_partition(&vids, partition_num);

        // Each VID should be in exactly one partition
        let mut total_vids = 0;
        for (part_id, part_vids) in &groups {
            assert!(*part_id >= 1 && *part_id <= partition_num);
            total_vids += part_vids.len();

            // Verify each VID hashes to this partition
            for vid in part_vids {
                let computed_part = byoridb_common::hash::compute_partition(*vid, partition_num);
                assert_eq!(computed_part, *part_id);
            }
        }

        assert_eq!(total_vids, vids.len());
    }

    #[test]
    fn test_group_edges_by_partition() {
        let edge_keys = vec![
            EdgeKey {
                src_vid: 1,
                dst_vid: 2,
                edge_type: "follow".to_string(),
                ranking: 0,
            },
            EdgeKey {
                src_vid: 3,
                dst_vid: 4,
                edge_type: "follow".to_string(),
                ranking: 0,
            },
        ];
        let partition_num = 2;

        let groups = DistributedQueryExecutor::group_edges_by_partition(&edge_keys, partition_num);

        let mut total_edges = 0;
        for (part_id, part_edges) in &groups {
            assert!(*part_id >= 1 && *part_id <= partition_num);
            total_edges += part_edges.len();
        }

        assert_eq!(total_edges, edge_keys.len());
    }

    #[test]
    fn test_default_config() {
        let config = DistributedQueryConfig::default();
        assert_eq!(config.max_parallel_requests, 100);
        assert_eq!(config.retry_count, 2);
        assert!(!config.read_from_followers);
    }
}

#[cfg(test)]
#[path = "distributed_completeness_tests.rs"]
mod completeness_tests;

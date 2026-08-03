// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Index management for vertices and edges
//!
//! This module provides indexing capabilities for fast property-based lookups
//! on vertices and edges. Indexes are stored as key-value pairs where:
//! - Key: Index prefix + property values + vertex/edge identifier
//! - Value: Empty (index key itself contains all needed information)

use crate::key::{IndexValue, KeyUtils};
use byoridb_kvstore::KVStore;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{OnceCell, RwLock};
use tracing::{debug, info, warn};

/// KV prefix under which index *definitions* are persisted.
/// Key: `__meta:index_def:{space_id}:{index_id}` → serde_json(IndexDef).
/// Definitions used to live only in the in-memory maps, so every fresh
/// `IndexManager` (per-query executor context, or a server restart) started
/// empty: INSERT stopped writing index entries and LOOKUP silently fell back
/// to full scans. Persisting the definitions and lazy-loading them on first
/// use makes index metadata survive both query boundaries and restarts.
pub const INDEX_DEF_PREFIX: &str = "__meta:index_def:";

fn index_def_key(space_id: u32, index_id: u32) -> Vec<u8> {
    format!("{}{}:{}", INDEX_DEF_PREFIX, space_id, index_id).into_bytes()
}

/// Index type (tag or edge)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexType {
    Tag,
    Edge,
}

/// Index definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexDef {
    pub id: u32,
    pub space_id: u32,
    pub index_name: String,
    pub index_type: IndexType,
    /// Tag ID or Edge Type ID
    pub schema_id: u32,
    /// Tag name or Edge type name
    pub schema_name: String,
    /// Field names in the index
    pub fields: Vec<String>,
    /// Field indices in the schema (for value extraction)
    pub field_indices: Vec<usize>,
}

/// Index scan result for tags
#[derive(Debug, Clone)]
pub struct TagIndexScanResult {
    pub vid: i64,
}

/// Index scan result for edges
#[derive(Debug, Clone)]
pub struct EdgeIndexScanResult {
    pub src_vid: i64,
    pub rank: i64,
    pub dst_vid: i64,
}

/// Index scan options
#[derive(Debug, Clone, Default)]
pub struct ScanOptions {
    /// Limit number of results (0 = no limit)
    pub limit: usize,
    /// Start from this property value (for range scans)
    pub start_values: Option<Vec<IndexValue>>,
    /// End at this property value (for range scans)
    pub end_values: Option<Vec<IndexValue>>,
    /// Include start boundary
    pub include_start: bool,
    /// Include end boundary
    pub include_end: bool,
}

/// Comparison operator for a single-field ordered index range lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeOperator {
    GreaterThan,
    GreaterThanOrEqual,
    LessThan,
    LessThanOrEqual,
}

fn prefix_successor(prefix: &[u8]) -> Option<Vec<u8>> {
    let mut end = prefix.to_vec();
    for index in (0..end.len()).rev() {
        if end[index] != u8::MAX {
            end[index] += 1;
            end.truncate(index + 1);
            return Some(end);
        }
    }
    None
}

/// Index manager for a storage node
pub struct IndexManager {
    kvstore: Arc<dyn KVStore>,
    /// Index definitions: (space_id, index_id) -> IndexDef
    indexes: RwLock<HashMap<(u32, u32), IndexDef>>,
    /// Index by name: (space_id, index_name) -> index_id
    index_names: RwLock<HashMap<(u32, String), u32>>,
    /// Next index ID
    next_index_id: RwLock<u32>,
    /// One-shot lazy load of persisted definitions (see [`INDEX_DEF_PREFIX`]).
    loaded: OnceCell<()>,
}

impl IndexManager {
    /// Create a new index manager
    ///
    /// Persisted index definitions are loaded lazily on first use, so a
    /// freshly constructed manager (per-query context or post-restart) sees
    /// every index created by earlier managers over the same kvstore.
    pub fn new(kvstore: Arc<dyn KVStore>) -> Self {
        Self {
            kvstore,
            indexes: RwLock::new(HashMap::new()),
            index_names: RwLock::new(HashMap::new()),
            next_index_id: RwLock::new(1),
            loaded: OnceCell::new(),
        }
    }

    /// Load persisted index definitions from the kvstore exactly once.
    ///
    /// Rebuilds the in-memory maps and advances `next_index_id` past the
    /// largest persisted id. Mutating entry points propagate errors; read
    /// paths (`get_*`/`list_*`) log and degrade to the unloaded view instead,
    /// because their signatures carry no error channel.
    async fn ensure_loaded(&self) -> Result<(), IndexError> {
        self.loaded
            .get_or_try_init(|| async {
                let entries = self
                    .kvstore
                    .scan_prefix(INDEX_DEF_PREFIX.as_bytes())
                    .await
                    .map_err(|e| IndexError::Storage(e.to_string()))?;

                let mut indexes = self.indexes.write().await;
                let mut names = self.index_names.write().await;
                let mut max_id = 0u32;
                let mut loaded = 0usize;
                for (key, value) in entries {
                    let def: IndexDef = match serde_json::from_slice(&value) {
                        Ok(def) => def,
                        Err(e) => {
                            // A corrupt definition must not take down every
                            // index — skip it loudly.
                            warn!(
                                "Skipping corrupt index definition at {:?}: {}",
                                String::from_utf8_lossy(&key),
                                e
                            );
                            continue;
                        }
                    };
                    max_id = max_id.max(def.id);
                    names.insert((def.space_id, def.index_name.clone()), def.id);
                    indexes.insert((def.space_id, def.id), def);
                    loaded += 1;
                }
                if loaded > 0 {
                    let mut next_id = self.next_index_id.write().await;
                    *next_id = (*next_id).max(max_id + 1);
                    info!("Loaded {} persisted index definition(s)", loaded);
                }
                Ok(())
            })
            .await
            .map(|_| ())
    }

    /// Read-path variant of [`ensure_loaded`]: log instead of propagating.
    async fn ensure_loaded_or_log(&self) {
        if let Err(e) = self.ensure_loaded().await {
            warn!("Failed to load persisted index definitions: {}", e);
        }
    }

    /// Persist one index definition to the kvstore.
    async fn persist_def(&self, def: &IndexDef) -> Result<(), IndexError> {
        let value = serde_json::to_vec(def).map_err(|e| IndexError::Storage(e.to_string()))?;
        self.kvstore
            .put(&index_def_key(def.space_id, def.id), &value)
            .await
            .map_err(|e| IndexError::Storage(e.to_string()))
    }

    /// Remove one persisted index definition from the kvstore.
    async fn delete_persisted_def(&self, space_id: u32, index_id: u32) -> Result<(), IndexError> {
        self.kvstore
            .delete(&index_def_key(space_id, index_id))
            .await
            .map_err(|e| IndexError::Storage(e.to_string()))
    }

    /// Create a new tag index
    pub async fn create_tag_index(
        &self,
        space_id: u32,
        index_name: String,
        tag_id: u32,
        tag_name: String,
        fields: Vec<String>,
        field_indices: Vec<usize>,
    ) -> Result<u32, IndexError> {
        info!(
            "Creating tag index '{}' on tag {}({}) with fields {:?}",
            index_name, tag_name, tag_id, fields
        );

        self.ensure_loaded().await?;

        // Check if index already exists
        {
            let names = self.index_names.read().await;
            if names.contains_key(&(space_id, index_name.clone())) {
                return Err(IndexError::IndexAlreadyExists(index_name));
            }
        }

        // Generate index ID
        let index_id = {
            let mut next_id = self.next_index_id.write().await;
            let id = *next_id;
            *next_id += 1;
            id
        };

        let index_def = IndexDef {
            id: index_id,
            space_id,
            index_name: index_name.clone(),
            index_type: IndexType::Tag,
            schema_id: tag_id,
            schema_name: tag_name,
            fields,
            field_indices,
        };

        // Persist first, then register in memory — a failed KV write must not
        // leave a definition that silently evaporates with this manager.
        self.persist_def(&index_def).await?;

        // Store index definition
        {
            let mut indexes = self.indexes.write().await;
            indexes.insert((space_id, index_id), index_def);
        }

        {
            let mut names = self.index_names.write().await;
            names.insert((space_id, index_name), index_id);
        }

        debug!("Created tag index with ID {}", index_id);
        Ok(index_id)
    }

    /// Create a new edge index
    pub async fn create_edge_index(
        &self,
        space_id: u32,
        index_name: String,
        edge_type_id: u32,
        edge_type_name: String,
        fields: Vec<String>,
        field_indices: Vec<usize>,
    ) -> Result<u32, IndexError> {
        info!(
            "Creating edge index '{}' on edge {}({}) with fields {:?}",
            index_name, edge_type_name, edge_type_id, fields
        );

        self.ensure_loaded().await?;

        // Check if index already exists
        {
            let names = self.index_names.read().await;
            if names.contains_key(&(space_id, index_name.clone())) {
                return Err(IndexError::IndexAlreadyExists(index_name));
            }
        }

        // Generate index ID
        let index_id = {
            let mut next_id = self.next_index_id.write().await;
            let id = *next_id;
            *next_id += 1;
            id
        };

        let index_def = IndexDef {
            id: index_id,
            space_id,
            index_name: index_name.clone(),
            index_type: IndexType::Edge,
            schema_id: edge_type_id,
            schema_name: edge_type_name,
            fields,
            field_indices,
        };

        // Persist first, then register in memory (see create_tag_index).
        self.persist_def(&index_def).await?;

        // Store index definition
        {
            let mut indexes = self.indexes.write().await;
            indexes.insert((space_id, index_id), index_def);
        }

        {
            let mut names = self.index_names.write().await;
            names.insert((space_id, index_name), index_id);
        }

        debug!("Created edge index with ID {}", index_id);
        Ok(index_id)
    }

    /// Drop an index
    ///
    /// Note: This only removes the index definition. To delete actual index entries,
    /// use `drop_index_with_data` which requires partition IDs.
    pub async fn drop_index(&self, space_id: u32, index_name: &str) -> Result<(), IndexError> {
        // Get index info before removing
        let index_def = self
            .get_index(space_id, index_name)
            .await
            .ok_or_else(|| IndexError::IndexNotFound(index_name.to_string()))?;

        info!("Dropping index '{}' (ID {})", index_name, index_def.id);

        // Remove the persisted definition first so a failure leaves the
        // index intact rather than resurrecting on the next load.
        self.delete_persisted_def(space_id, index_def.id).await?;

        // Remove from name mapping
        {
            let mut names = self.index_names.write().await;
            names.remove(&(space_id, index_name.to_string()));
        }

        // Remove from index definitions
        {
            let mut indexes = self.indexes.write().await;
            indexes.remove(&(space_id, index_def.id));
        }

        debug!("Dropped index '{}' (ID {})", index_name, index_def.id);
        Ok(())
    }

    /// Drop an index and delete all its entries from KVStore
    ///
    /// This method requires the list of partition IDs to clean up index entries.
    pub async fn drop_index_with_data(
        &self,
        space_id: u32,
        index_name: &str,
        partition_ids: &[u32],
    ) -> Result<(), IndexError> {
        // Get index info before removing
        let index_def = self
            .get_index(space_id, index_name)
            .await
            .ok_or_else(|| IndexError::IndexNotFound(index_name.to_string()))?;

        info!(
            "Dropping index '{}' (ID {}) with data cleanup for {} partitions",
            index_name,
            index_def.id,
            partition_ids.len()
        );

        // Delete all index entries from KVStore
        let deleted_count = self
            .delete_all_index_entries(&index_def, partition_ids)
            .await?;

        // Remove the persisted definition (see drop_index).
        self.delete_persisted_def(space_id, index_def.id).await?;

        // Remove from name mapping
        {
            let mut names = self.index_names.write().await;
            names.remove(&(space_id, index_name.to_string()));
        }

        // Remove from index definitions
        {
            let mut indexes = self.indexes.write().await;
            indexes.remove(&(space_id, index_def.id));
        }

        info!(
            "Dropped index '{}' (ID {}), deleted {} entries",
            index_name, index_def.id, deleted_count
        );
        Ok(())
    }

    /// Delete all index entries for a given index across specified partitions
    async fn delete_all_index_entries(
        &self,
        index_def: &IndexDef,
        partition_ids: &[u32],
    ) -> Result<usize, IndexError> {
        let mut total_deleted = 0;

        for &part_id in partition_ids {
            let prefix = match index_def.index_type {
                IndexType::Tag => KeyUtils::tag_index_prefix(part_id, index_def.id),
                IndexType::Edge => KeyUtils::edge_index_prefix(part_id, index_def.id),
            };

            // Scan all keys with this prefix
            let entries = self
                .kvstore
                .scan_prefix(&prefix)
                .await
                .map_err(|e| IndexError::Storage(e.to_string()))?;

            // Delete each entry
            for (key, _) in &entries {
                self.kvstore
                    .delete(key)
                    .await
                    .map_err(|e| IndexError::Storage(e.to_string()))?;
            }

            total_deleted += entries.len();
        }

        Ok(total_deleted)
    }

    /// Get an index definition by name
    pub async fn get_index(&self, space_id: u32, index_name: &str) -> Option<IndexDef> {
        self.ensure_loaded_or_log().await;
        let names = self.index_names.read().await;
        let index_id = names.get(&(space_id, index_name.to_string()))?;

        let indexes = self.indexes.read().await;
        indexes.get(&(space_id, *index_id)).cloned()
    }

    /// Get an index definition by ID
    pub async fn get_index_by_id(&self, space_id: u32, index_id: u32) -> Option<IndexDef> {
        self.ensure_loaded_or_log().await;
        let indexes = self.indexes.read().await;
        indexes.get(&(space_id, index_id)).cloned()
    }

    /// List all indexes in a space
    pub async fn list_indexes(&self, space_id: u32) -> Vec<IndexDef> {
        self.ensure_loaded_or_log().await;
        let indexes = self.indexes.read().await;
        indexes
            .values()
            .filter(|idx| idx.space_id == space_id)
            .cloned()
            .collect()
    }

    /// List tag indexes in a space
    pub async fn list_tag_indexes(&self, space_id: u32) -> Vec<IndexDef> {
        self.ensure_loaded_or_log().await;
        let indexes = self.indexes.read().await;
        indexes
            .values()
            .filter(|idx| idx.space_id == space_id && idx.index_type == IndexType::Tag)
            .cloned()
            .collect()
    }

    /// List edge indexes in a space
    pub async fn list_edge_indexes(&self, space_id: u32) -> Vec<IndexDef> {
        self.ensure_loaded_or_log().await;
        let indexes = self.indexes.read().await;
        indexes
            .values()
            .filter(|idx| idx.space_id == space_id && idx.index_type == IndexType::Edge)
            .cloned()
            .collect()
    }

    /// Insert a tag index entry
    pub async fn insert_tag_index(
        &self,
        part_id: u32,
        index_id: u32,
        prop_values: &[IndexValue],
        vid: i64,
    ) -> Result<(), IndexError> {
        let key = KeyUtils::tag_index_key(part_id, index_id, prop_values, vid);
        self.kvstore
            .put(&key, &[])
            .await
            .map_err(|e| IndexError::Storage(e.to_string()))?;
        Ok(())
    }

    /// Delete a tag index entry
    pub async fn delete_tag_index(
        &self,
        part_id: u32,
        index_id: u32,
        prop_values: &[IndexValue],
        vid: i64,
    ) -> Result<(), IndexError> {
        let key = KeyUtils::tag_index_key(part_id, index_id, prop_values, vid);
        self.kvstore
            .delete(&key)
            .await
            .map_err(|e| IndexError::Storage(e.to_string()))?;
        Ok(())
    }

    /// Insert an edge index entry
    pub async fn insert_edge_index(
        &self,
        part_id: u32,
        index_id: u32,
        prop_values: &[IndexValue],
        src_vid: i64,
        rank: i64,
        dst_vid: i64,
    ) -> Result<(), IndexError> {
        let key = KeyUtils::edge_index_key(part_id, index_id, prop_values, src_vid, rank, dst_vid);
        self.kvstore
            .put(&key, &[])
            .await
            .map_err(|e| IndexError::Storage(e.to_string()))?;
        Ok(())
    }

    /// Delete an edge index entry
    pub async fn delete_edge_index(
        &self,
        part_id: u32,
        index_id: u32,
        prop_values: &[IndexValue],
        src_vid: i64,
        rank: i64,
        dst_vid: i64,
    ) -> Result<(), IndexError> {
        let key = KeyUtils::edge_index_key(part_id, index_id, prop_values, src_vid, rank, dst_vid);
        self.kvstore
            .delete(&key)
            .await
            .map_err(|e| IndexError::Storage(e.to_string()))?;
        Ok(())
    }

    /// Count index entries grouped by their (single) indexed value, scanning the
    /// whole index without decoding any vertex blobs. Accelerates `GROUP BY
    /// <indexed_prop>` + COUNT(*): O(index entries with tiny keys) instead of
    /// O(vertices with full blob decode). Single-field indexes only (multi-field
    /// errors → caller falls back to full scan).
    pub async fn tag_index_group_counts(
        &self,
        part_id: u32,
        index_def: &IndexDef,
    ) -> Result<Vec<(IndexValue, u64)>, IndexError> {
        if index_def.fields.len() != 1 {
            return Err(IndexError::Storage(
                "group-count optimization requires a single-field index".to_string(),
            ));
        }
        let prefix = KeyUtils::tag_index_prefix(part_id, index_def.id);
        let plen = prefix.len();
        let entries = self
            .kvstore
            .scan_prefix(&prefix)
            .await
            .map_err(|e| IndexError::Storage(e.to_string()))?;
        // Group by the encoded value bytes — Float isn't Hash/Eq, and the
        // encoding is canonical per IndexValue::encode, so it's a valid key.
        let mut counts: HashMap<Vec<u8>, (IndexValue, u64)> = HashMap::new();
        for (key, _) in &entries {
            if key.len() <= plen {
                continue;
            }
            if let Some((value, consumed)) = IndexValue::decode(&key[plen..]) {
                let vbytes = key[plen..plen + consumed].to_vec();
                counts.entry(vbytes).or_insert((value, 0)).1 += 1;
            }
        }
        Ok(counts.into_values().collect())
    }

    /// Scan a tag index with given prefix values
    pub async fn scan_tag_index(
        &self,
        part_id: u32,
        index_def: &IndexDef,
        prefix_values: &[IndexValue],
        options: &ScanOptions,
    ) -> Result<Vec<TagIndexScanResult>, IndexError> {
        let prefix = KeyUtils::tag_index_prefix_with_values(part_id, index_def.id, prefix_values);
        let limit = (options.limit > 0).then_some(options.limit);

        let entries = if options.start_values.is_none() && options.end_values.is_none() {
            self.kvstore.scan_prefix_limited(&prefix, limit).await
        } else {
            let prefix_end = prefix_successor(&prefix).ok_or_else(|| {
                IndexError::InvalidValue("index prefix has no upper bound".to_string())
            })?;
            let start = match options.start_values.as_ref() {
                Some(values) => {
                    let mut all_values = prefix_values.to_vec();
                    all_values.extend(values.iter().cloned());
                    let boundary =
                        KeyUtils::tag_index_prefix_with_values(part_id, index_def.id, &all_values);
                    if options.include_start {
                        boundary
                    } else {
                        prefix_successor(&boundary).unwrap_or_else(|| prefix_end.clone())
                    }
                }
                None => prefix.clone(),
            };
            let end = match options.end_values.as_ref() {
                Some(values) => {
                    let mut all_values = prefix_values.to_vec();
                    all_values.extend(values.iter().cloned());
                    let boundary =
                        KeyUtils::tag_index_prefix_with_values(part_id, index_def.id, &all_values);
                    if options.include_end {
                        prefix_successor(&boundary).unwrap_or_else(|| prefix_end.clone())
                    } else {
                        boundary
                    }
                }
                None => prefix_end,
            };
            self.kvstore.scan_range(&start, &end, limit).await
        }
        .map_err(|e| IndexError::Storage(e.to_string()))?;

        let mut results: Vec<TagIndexScanResult> = entries
            .iter()
            .filter_map(|(key, _)| {
                KeyUtils::parse_tag_index_vid(key, 0).map(|vid| TagIndexScanResult { vid })
            })
            .collect();

        // Apply limit
        if options.limit > 0 && results.len() > options.limit {
            results.truncate(options.limit);
        }

        Ok(results)
    }

    /// Lookup a single-field tag index using an ordered comparison.
    pub async fn lookup_tag_range(
        &self,
        part_id: u32,
        index_def: &IndexDef,
        value: IndexValue,
        operator: RangeOperator,
        limit: usize,
    ) -> Result<Vec<i64>, IndexError> {
        if index_def.fields.len() != 1 {
            return Err(IndexError::InvalidValue(
                "range lookup requires a single-field index".to_string(),
            ));
        }
        if matches!(value, IndexValue::Null | IndexValue::String(_))
            || matches!(value, IndexValue::Float(number) if number.is_nan() || number == 0.0)
        {
            return Err(IndexError::InvalidValue(
                "ordered range lookup requires bool, int, or a non-zero non-NaN float boundary"
                    .to_string(),
            ));
        }

        let (type_min, type_max) = match &value {
            IndexValue::Bool(_) => (IndexValue::Bool(false), IndexValue::Bool(true)),
            IndexValue::Int(_) => (IndexValue::Int(i64::MIN), IndexValue::Int(i64::MAX)),
            IndexValue::Float(_) => (
                IndexValue::Float(f64::NEG_INFINITY),
                IndexValue::Float(f64::INFINITY),
            ),
            IndexValue::Null | IndexValue::String(_) => unreachable!(),
        };
        let mut options = ScanOptions {
            limit,
            start_values: Some(vec![type_min]),
            end_values: Some(vec![type_max]),
            include_start: true,
            include_end: true,
        };
        match operator {
            RangeOperator::GreaterThan => {
                options.start_values = Some(vec![value]);
                options.include_start = false;
            }
            RangeOperator::GreaterThanOrEqual => {
                options.start_values = Some(vec![value]);
                options.include_start = true;
            }
            RangeOperator::LessThan => {
                options.end_values = Some(vec![value]);
                options.include_end = false;
            }
            RangeOperator::LessThanOrEqual => {
                options.end_values = Some(vec![value]);
                options.include_end = true;
            }
        }

        Ok(self
            .scan_tag_index(part_id, index_def, &[], &options)
            .await?
            .into_iter()
            .map(|result| result.vid)
            .collect())
    }

    /// Scan an edge index with given prefix values
    pub async fn scan_edge_index(
        &self,
        part_id: u32,
        index_def: &IndexDef,
        prefix_values: &[IndexValue],
        options: &ScanOptions,
    ) -> Result<Vec<EdgeIndexScanResult>, IndexError> {
        let prefix = KeyUtils::edge_index_prefix_with_values(part_id, index_def.id, prefix_values);

        // Calculate property value length for parsing
        let prop_len: usize = prefix_values.iter().map(|v| v.encoded_len()).sum();

        let entries = self
            .kvstore
            .scan_prefix(&prefix)
            .await
            .map_err(|e| IndexError::Storage(e.to_string()))?;

        let mut results: Vec<EdgeIndexScanResult> = entries
            .iter()
            .filter_map(|(key, _)| {
                KeyUtils::parse_edge_index_edge(key, prop_len).map(|(src_vid, rank, dst_vid)| {
                    EdgeIndexScanResult {
                        src_vid,
                        rank,
                        dst_vid,
                    }
                })
            })
            .collect();

        // Apply limit
        if options.limit > 0 && results.len() > options.limit {
            results.truncate(options.limit);
        }

        Ok(results)
    }

    /// Lookup tag by exact property values
    pub async fn lookup_tag(
        &self,
        part_id: u32,
        index_def: &IndexDef,
        values: &[IndexValue],
        limit: usize,
    ) -> Result<Vec<i64>, IndexError> {
        let results = self
            .scan_tag_index(
                part_id,
                index_def,
                values,
                &ScanOptions {
                    limit,
                    ..Default::default()
                },
            )
            .await?;
        Ok(results.into_iter().map(|r| r.vid).collect())
    }

    /// Lookup edge by exact property values
    pub async fn lookup_edge(
        &self,
        part_id: u32,
        index_def: &IndexDef,
        values: &[IndexValue],
        limit: usize,
    ) -> Result<Vec<(i64, i64, i64)>, IndexError> {
        let results = self
            .scan_edge_index(
                part_id,
                index_def,
                values,
                &ScanOptions {
                    limit,
                    ..Default::default()
                },
            )
            .await?;
        Ok(results
            .into_iter()
            .map(|r| (r.src_vid, r.rank, r.dst_vid))
            .collect())
    }

    /// Get indexes for a tag (by tag_id)
    pub async fn get_indexes_for_tag(&self, space_id: u32, tag_id: u32) -> Vec<IndexDef> {
        let indexes = self.indexes.read().await;
        indexes
            .values()
            .filter(|idx| {
                idx.space_id == space_id
                    && idx.index_type == IndexType::Tag
                    && idx.schema_id == tag_id
            })
            .cloned()
            .collect()
    }

    /// Get indexes for an edge type
    pub async fn get_indexes_for_edge(&self, space_id: u32, edge_type: u32) -> Vec<IndexDef> {
        let indexes = self.indexes.read().await;
        indexes
            .values()
            .filter(|idx| {
                idx.space_id == space_id
                    && idx.index_type == IndexType::Edge
                    && idx.schema_id == edge_type
            })
            .cloned()
            .collect()
    }
}

/// Index-related errors
#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("Index already exists: {0}")]
    IndexAlreadyExists(String),

    #[error("Index not found: {0}")]
    IndexNotFound(String),

    #[error("Field not found: {0}")]
    FieldNotFound(String),

    #[error("Storage error: {0}")]
    Storage(String),

    #[error("Invalid index value: {0}")]
    InvalidValue(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use byoridb_kvstore::MemoryKVStore;

    /// Restart regression (PLAN.md 선재 이슈): definitions must survive a
    /// fresh IndexManager over the same kvstore — the per-query-context and
    /// server-restart cases share this exact mechanism.
    #[tokio::test]
    async fn test_index_definitions_survive_manager_recreation() {
        let kvstore: Arc<dyn KVStore> = Arc::new(MemoryKVStore::new());

        let manager = IndexManager::new(kvstore.clone());
        let tag_idx_id = manager
            .create_tag_index(
                1,
                "person_name_idx".to_string(),
                10,
                "person".to_string(),
                vec!["name".to_string()],
                vec![0],
            )
            .await
            .unwrap();
        manager
            .create_edge_index(
                1,
                "knows_since_idx".to_string(),
                20,
                "knows".to_string(),
                vec!["since".to_string()],
                vec![0],
            )
            .await
            .unwrap();
        drop(manager);

        // "Restart": brand-new manager over the same kvstore.
        let reborn = IndexManager::new(kvstore.clone());
        let def = reborn
            .get_index(1, "person_name_idx")
            .await
            .expect("tag index definition must survive manager recreation");
        assert_eq!(def.id, tag_idx_id);
        assert_eq!(def.index_type, IndexType::Tag);
        assert_eq!(def.schema_name, "person");
        assert_eq!(def.fields, vec!["name".to_string()]);

        assert_eq!(reborn.list_tag_indexes(1).await.len(), 1);
        assert_eq!(reborn.list_edge_indexes(1).await.len(), 1);

        // ID allocation must continue past persisted ids, not collide.
        let new_id = reborn
            .create_tag_index(
                1,
                "person_age_idx".to_string(),
                10,
                "person".to_string(),
                vec!["age".to_string()],
                vec![0],
            )
            .await
            .unwrap();
        assert!(
            new_id > 2,
            "new id {} must not collide with persisted ids",
            new_id
        );

        // Duplicate-name check must also see persisted definitions.
        let dup = reborn
            .create_tag_index(
                1,
                "person_name_idx".to_string(),
                10,
                "person".to_string(),
                vec!["name".to_string()],
                vec![0],
            )
            .await;
        assert!(matches!(dup, Err(IndexError::IndexAlreadyExists(_))));
    }

    /// Dropped definitions must stay dropped after a manager recreation.
    #[tokio::test]
    async fn test_dropped_index_does_not_resurrect_after_recreation() {
        let kvstore: Arc<dyn KVStore> = Arc::new(MemoryKVStore::new());

        let manager = IndexManager::new(kvstore.clone());
        manager
            .create_tag_index(
                1,
                "tmp_idx".to_string(),
                10,
                "person".to_string(),
                vec!["name".to_string()],
                vec![0],
            )
            .await
            .unwrap();
        manager.drop_index(1, "tmp_idx").await.unwrap();
        drop(manager);

        let reborn = IndexManager::new(kvstore);
        assert!(
            reborn.get_index(1, "tmp_idx").await.is_none(),
            "dropped index must not resurrect from persisted state"
        );
        assert!(reborn.list_indexes(1).await.is_empty());
    }

    #[tokio::test]
    async fn test_create_tag_index() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let manager = IndexManager::new(kvstore);

        let index_id = manager
            .create_tag_index(
                1,
                "person_name_idx".to_string(),
                10, // tag_id
                "person".to_string(),
                vec!["name".to_string()],
                vec![0],
            )
            .await
            .unwrap();

        assert_eq!(index_id, 1);

        let index = manager.get_index(1, "person_name_idx").await.unwrap();
        assert_eq!(index.index_name, "person_name_idx");
        assert_eq!(index.schema_id, 10);
        assert_eq!(index.schema_name, "person");
        assert_eq!(index.fields, vec!["name".to_string()]);
    }

    #[tokio::test]
    async fn test_tag_index_group_counts() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let manager = IndexManager::new(kvstore);
        manager
            .create_tag_index(
                1,
                "prod_ch_idx".to_string(),
                10,
                "product".to_string(),
                vec!["channel".to_string()],
                vec![0],
            )
            .await
            .unwrap();
        let index_def = manager.get_index(1, "prod_ch_idx").await.unwrap();
        // channel distribution: a×3, b×1, c×2
        for (ch, vid) in [("a", 1), ("a", 2), ("a", 3), ("b", 4), ("c", 5), ("c", 6)] {
            manager
                .insert_tag_index(1, index_def.id, &[IndexValue::String(ch.to_string())], vid)
                .await
                .unwrap();
        }
        let counts = manager.tag_index_group_counts(1, &index_def).await.unwrap();
        let mut m = std::collections::HashMap::new();
        for (iv, n) in counts {
            if let IndexValue::String(s) = iv {
                m.insert(s, n);
            }
        }
        assert_eq!(m.get("a"), Some(&3), "a counted 3");
        assert_eq!(m.get("b"), Some(&1), "b counted 1");
        assert_eq!(m.get("c"), Some(&2), "c counted 2");
        assert_eq!(m.len(), 3, "exactly 3 distinct values");
    }

    #[tokio::test]
    async fn test_tag_index_group_counts_rejects_multifield() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let manager = IndexManager::new(kvstore);
        manager
            .create_tag_index(
                1,
                "multi".to_string(),
                10,
                "t".to_string(),
                vec!["a".to_string(), "b".to_string()],
                vec![0, 1],
            )
            .await
            .unwrap();
        let def = manager.get_index(1, "multi").await.unwrap();
        // multi-field index isn't a single-value group key → error (caller
        // falls back to full scan).
        assert!(manager.tag_index_group_counts(1, &def).await.is_err());
    }

    #[tokio::test]
    async fn test_create_edge_index() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let manager = IndexManager::new(kvstore);

        let index_id = manager
            .create_edge_index(
                1,
                "knows_degree_idx".to_string(),
                20, // edge_type_id
                "knows".to_string(),
                vec!["degree".to_string()],
                vec![0],
            )
            .await
            .unwrap();

        assert_eq!(index_id, 1);

        let indexes = manager.list_edge_indexes(1).await;
        assert_eq!(indexes.len(), 1);
        assert_eq!(indexes[0].index_name, "knows_degree_idx");
        assert_eq!(indexes[0].schema_name, "knows");
    }

    #[tokio::test]
    async fn test_duplicate_index() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let manager = IndexManager::new(kvstore);

        manager
            .create_tag_index(
                1,
                "idx1".to_string(),
                10,
                "tag1".to_string(),
                vec!["name".to_string()],
                vec![0],
            )
            .await
            .unwrap();

        let result = manager
            .create_tag_index(
                1,
                "idx1".to_string(),
                10,
                "tag1".to_string(),
                vec!["name".to_string()],
                vec![0],
            )
            .await;

        assert!(matches!(result, Err(IndexError::IndexAlreadyExists(_))));
    }

    #[tokio::test]
    async fn test_drop_index() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let manager = IndexManager::new(kvstore);

        manager
            .create_tag_index(
                1,
                "idx1".to_string(),
                10,
                "tag1".to_string(),
                vec!["name".to_string()],
                vec![0],
            )
            .await
            .unwrap();

        manager.drop_index(1, "idx1").await.unwrap();

        let index = manager.get_index(1, "idx1").await;
        assert!(index.is_none());
    }

    #[tokio::test]
    async fn test_insert_and_lookup_tag_index() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let manager = IndexManager::new(kvstore);

        let index_id = manager
            .create_tag_index(
                1,
                "person_name_idx".to_string(),
                10,
                "person".to_string(),
                vec!["name".to_string()],
                vec![0],
            )
            .await
            .unwrap();

        // Insert index entries
        manager
            .insert_tag_index(1, index_id, &[IndexValue::String("Alice".to_string())], 100)
            .await
            .unwrap();
        manager
            .insert_tag_index(1, index_id, &[IndexValue::String("Alice".to_string())], 101)
            .await
            .unwrap();
        manager
            .insert_tag_index(1, index_id, &[IndexValue::String("Bob".to_string())], 102)
            .await
            .unwrap();

        // Lookup
        let index_def = manager.get_index(1, "person_name_idx").await.unwrap();
        let results = manager
            .lookup_tag(
                1,
                &index_def,
                &[IndexValue::String("Alice".to_string())],
                100,
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 2);
        assert!(results.contains(&100));
        assert!(results.contains(&101));
    }

    #[tokio::test]
    async fn test_lookup_tag_range_boundaries_and_null_exclusion() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let manager = IndexManager::new(kvstore);
        let index_id = manager
            .create_tag_index(
                1,
                "person_age_idx".to_string(),
                10,
                "person".to_string(),
                vec!["age".to_string()],
                vec![0],
            )
            .await
            .unwrap();
        let index_def = manager.get_index(1, "person_age_idx").await.unwrap();

        for (value, vid) in [(-10, 1), (0, 2), (30, 3), (30, 4), (50, 5)] {
            manager
                .insert_tag_index(1, index_id, &[IndexValue::Int(value)], vid)
                .await
                .unwrap();
        }
        manager
            .insert_tag_index(1, index_id, &[IndexValue::Null], 99)
            .await
            .unwrap();

        for (operator, expected) in [
            (RangeOperator::GreaterThan, vec![5]),
            (RangeOperator::GreaterThanOrEqual, vec![3, 4, 5]),
            (RangeOperator::LessThan, vec![1, 2]),
            (RangeOperator::LessThanOrEqual, vec![1, 2, 3, 4]),
        ] {
            let results = manager
                .lookup_tag_range(1, &index_def, IndexValue::Int(30), operator, 100)
                .await
                .unwrap();
            assert_eq!(results, expected, "operator: {operator:?}");
            assert!(!results.contains(&99), "NULL must not satisfy a range");
        }

        let limited = manager
            .lookup_tag_range(
                1,
                &index_def,
                IndexValue::Int(30),
                RangeOperator::LessThanOrEqual,
                2,
            )
            .await
            .unwrap();
        assert_eq!(limited, vec![1, 2]);
    }

    #[tokio::test]
    async fn test_lookup_tag_range_float_and_bool_domains() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let manager = IndexManager::new(kvstore);
        let float_id = manager
            .create_tag_index(
                1,
                "score_idx".to_string(),
                10,
                "result".to_string(),
                vec!["score".to_string()],
                vec![0],
            )
            .await
            .unwrap();
        let bool_id = manager
            .create_tag_index(
                1,
                "active_idx".to_string(),
                11,
                "result".to_string(),
                vec!["active".to_string()],
                vec![1],
            )
            .await
            .unwrap();
        let float_def = manager.get_index(1, "score_idx").await.unwrap();
        let bool_def = manager.get_index(1, "active_idx").await.unwrap();

        for (value, vid) in [(-2.5, 1), (-0.0, 2), (0.0, 3), (1.5, 4), (3.0, 5)] {
            manager
                .insert_tag_index(1, float_id, &[IndexValue::Float(value)], vid)
                .await
                .unwrap();
        }
        for (value, vid) in [(false, 10), (true, 11)] {
            manager
                .insert_tag_index(1, bool_id, &[IndexValue::Bool(value)], vid)
                .await
                .unwrap();
        }

        assert_eq!(
            manager
                .lookup_tag_range(
                    1,
                    &float_def,
                    IndexValue::Float(1.5),
                    RangeOperator::LessThanOrEqual,
                    100,
                )
                .await
                .unwrap(),
            vec![1, 2, 3, 4]
        );
        assert!(manager
            .lookup_tag_range(
                1,
                &float_def,
                IndexValue::Float(0.0),
                RangeOperator::GreaterThan,
                100,
            )
            .await
            .is_err());
        assert_eq!(
            manager
                .lookup_tag_range(
                    1,
                    &bool_def,
                    IndexValue::Bool(false),
                    RangeOperator::GreaterThan,
                    100,
                )
                .await
                .unwrap(),
            vec![11]
        );
    }

    #[tokio::test]
    async fn test_insert_and_lookup_edge_index() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let manager = IndexManager::new(kvstore);

        let index_id = manager
            .create_edge_index(
                1,
                "knows_since_idx".to_string(),
                20,
                "knows".to_string(),
                vec!["since".to_string()],
                vec![0],
            )
            .await
            .unwrap();

        // Insert index entries
        manager
            .insert_edge_index(1, index_id, &[IndexValue::Int(2020)], 100, 0, 200)
            .await
            .unwrap();
        manager
            .insert_edge_index(1, index_id, &[IndexValue::Int(2020)], 101, 0, 201)
            .await
            .unwrap();
        manager
            .insert_edge_index(1, index_id, &[IndexValue::Int(2021)], 102, 0, 202)
            .await
            .unwrap();

        // Lookup
        let index_def = manager.get_index(1, "knows_since_idx").await.unwrap();
        let results = manager
            .lookup_edge(1, &index_def, &[IndexValue::Int(2020)], 100)
            .await
            .unwrap();

        assert_eq!(results.len(), 2);
        assert!(results.contains(&(100, 0, 200)));
        assert!(results.contains(&(101, 0, 201)));
    }

    #[tokio::test]
    async fn test_composite_index() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let manager = IndexManager::new(kvstore);

        let index_id = manager
            .create_tag_index(
                1,
                "person_name_age_idx".to_string(),
                10,
                "person".to_string(),
                vec!["name".to_string(), "age".to_string()],
                vec![0, 1],
            )
            .await
            .unwrap();

        // Insert entries
        manager
            .insert_tag_index(
                1,
                index_id,
                &[IndexValue::String("Alice".to_string()), IndexValue::Int(30)],
                100,
            )
            .await
            .unwrap();
        manager
            .insert_tag_index(
                1,
                index_id,
                &[IndexValue::String("Alice".to_string()), IndexValue::Int(25)],
                101,
            )
            .await
            .unwrap();
        manager
            .insert_tag_index(
                1,
                index_id,
                &[IndexValue::String("Bob".to_string()), IndexValue::Int(30)],
                102,
            )
            .await
            .unwrap();

        // Lookup with full composite key
        let index_def = manager.get_index(1, "person_name_age_idx").await.unwrap();
        let results = manager
            .lookup_tag(
                1,
                &index_def,
                &[IndexValue::String("Alice".to_string()), IndexValue::Int(30)],
                100,
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], 100);

        // Lookup with prefix (name only)
        let results = manager
            .lookup_tag(
                1,
                &index_def,
                &[IndexValue::String("Alice".to_string())],
                100,
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 2);
        assert!(results.contains(&100));
        assert!(results.contains(&101));
    }

    #[tokio::test]
    async fn test_get_indexes_for_tag() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let manager = IndexManager::new(kvstore);

        manager
            .create_tag_index(
                1,
                "idx1".to_string(),
                10,
                "tag1".to_string(),
                vec!["name".to_string()],
                vec![0],
            )
            .await
            .unwrap();
        manager
            .create_tag_index(
                1,
                "idx2".to_string(),
                10,
                "tag1".to_string(),
                vec!["age".to_string()],
                vec![1],
            )
            .await
            .unwrap();
        manager
            .create_tag_index(
                1,
                "idx3".to_string(),
                20,
                "tag2".to_string(),
                vec!["title".to_string()],
                vec![0],
            )
            .await
            .unwrap();

        let indexes = manager.get_indexes_for_tag(1, 10).await;
        assert_eq!(indexes.len(), 2);
    }

    #[tokio::test]
    async fn test_drop_index_with_data() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let manager = IndexManager::new(kvstore.clone());

        // Create index
        let index_id = manager
            .create_tag_index(
                1,
                "person_name_idx".to_string(),
                10,
                "person".to_string(),
                vec!["name".to_string()],
                vec![0],
            )
            .await
            .unwrap();

        // Insert index entries across multiple partitions
        manager
            .insert_tag_index(1, index_id, &[IndexValue::String("Alice".to_string())], 100)
            .await
            .unwrap();
        manager
            .insert_tag_index(1, index_id, &[IndexValue::String("Bob".to_string())], 101)
            .await
            .unwrap();
        manager
            .insert_tag_index(
                2,
                index_id,
                &[IndexValue::String("Charlie".to_string())],
                102,
            )
            .await
            .unwrap();

        // Verify entries exist
        let index_def = manager.get_index(1, "person_name_idx").await.unwrap();
        let results1 = manager
            .lookup_tag(
                1,
                &index_def,
                &[IndexValue::String("Alice".to_string())],
                100,
            )
            .await
            .unwrap();
        assert_eq!(results1.len(), 1);

        // Drop index with data cleanup for partitions 1 and 2
        manager
            .drop_index_with_data(1, "person_name_idx", &[1, 2])
            .await
            .unwrap();

        // Verify index definition is gone
        let index = manager.get_index(1, "person_name_idx").await;
        assert!(index.is_none());

        // Verify KVStore entries are deleted (by checking scan returns empty)
        let prefix1 = KeyUtils::tag_index_prefix(1, index_id);
        let entries1 = kvstore.scan_prefix(&prefix1).await.unwrap();
        assert!(entries1.is_empty(), "Partition 1 entries should be deleted");

        let prefix2 = KeyUtils::tag_index_prefix(2, index_id);
        let entries2 = kvstore.scan_prefix(&prefix2).await.unwrap();
        assert!(entries2.is_empty(), "Partition 2 entries should be deleted");
    }
}

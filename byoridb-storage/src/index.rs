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
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Index type (tag or edge)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexType {
    Tag,
    Edge,
}

/// Index definition
#[derive(Debug, Clone)]
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

/// Index manager for a storage node
pub struct IndexManager {
    kvstore: Arc<dyn KVStore>,
    /// Index definitions: (space_id, index_id) -> IndexDef
    indexes: RwLock<HashMap<(u32, u32), IndexDef>>,
    /// Index by name: (space_id, index_name) -> index_id
    index_names: RwLock<HashMap<(u32, String), u32>>,
    /// Next index ID
    next_index_id: RwLock<u32>,
}

impl IndexManager {
    /// Create a new index manager
    pub fn new(kvstore: Arc<dyn KVStore>) -> Self {
        Self {
            kvstore,
            indexes: RwLock::new(HashMap::new()),
            index_names: RwLock::new(HashMap::new()),
            next_index_id: RwLock::new(1),
        }
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
        let names = self.index_names.read().await;
        let index_id = names.get(&(space_id, index_name.to_string()))?;

        let indexes = self.indexes.read().await;
        indexes.get(&(space_id, *index_id)).cloned()
    }

    /// Get an index definition by ID
    pub async fn get_index_by_id(&self, space_id: u32, index_id: u32) -> Option<IndexDef> {
        let indexes = self.indexes.read().await;
        indexes.get(&(space_id, index_id)).cloned()
    }

    /// List all indexes in a space
    pub async fn list_indexes(&self, space_id: u32) -> Vec<IndexDef> {
        let indexes = self.indexes.read().await;
        indexes
            .values()
            .filter(|idx| idx.space_id == space_id)
            .cloned()
            .collect()
    }

    /// List tag indexes in a space
    pub async fn list_tag_indexes(&self, space_id: u32) -> Vec<IndexDef> {
        let indexes = self.indexes.read().await;
        indexes
            .values()
            .filter(|idx| idx.space_id == space_id && idx.index_type == IndexType::Tag)
            .cloned()
            .collect()
    }

    /// List edge indexes in a space
    pub async fn list_edge_indexes(&self, space_id: u32) -> Vec<IndexDef> {
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

    /// Scan a tag index with given prefix values
    pub async fn scan_tag_index(
        &self,
        part_id: u32,
        index_def: &IndexDef,
        prefix_values: &[IndexValue],
        options: &ScanOptions,
    ) -> Result<Vec<TagIndexScanResult>, IndexError> {
        let prefix = KeyUtils::tag_index_prefix_with_values(part_id, index_def.id, prefix_values);

        // Calculate property value length for parsing
        let prop_len: usize = prefix_values.iter().map(|v| v.encoded_len()).sum();

        let entries = self
            .kvstore
            .scan_prefix(&prefix)
            .await
            .map_err(|e| IndexError::Storage(e.to_string()))?;

        let mut results: Vec<TagIndexScanResult> = entries
            .iter()
            .filter_map(|(key, _)| {
                KeyUtils::parse_tag_index_vid(key, prop_len).map(|vid| TagIndexScanResult { vid })
            })
            .collect();

        // Apply limit
        if options.limit > 0 && results.len() > options.limit {
            results.truncate(options.limit);
        }

        Ok(results)
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

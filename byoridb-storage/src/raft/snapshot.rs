// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Raft snapshot management
//!
//! This module handles snapshot creation, storage, and installation for Raft consensus.
//! Snapshots are used to:
//! - Compact the log by removing old entries
//! - Catch up lagging followers quickly
//! - Recover state after crashes

use super::types::{LogIndex, Term};
use byoridb_kvstore::KVStore;

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, info};

/// Key prefix for snapshot metadata
const SNAPSHOT_META_PREFIX: &[u8] = b"raft:snapshot:meta:";
/// Key prefix for snapshot data chunks
const SNAPSHOT_DATA_PREFIX: &[u8] = b"raft:snapshot:data:";

/// Snapshot metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotMeta {
    /// The last log index included in this snapshot
    pub last_included_index: LogIndex,
    /// The term of the last log entry included in this snapshot
    pub last_included_term: Term,
    /// Size of the snapshot data in bytes
    pub size: u64,
    /// Number of chunks the snapshot is split into
    pub chunk_count: u32,
    /// Timestamp when the snapshot was created
    pub created_at: u64,
}

/// A chunk of snapshot data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotChunk {
    /// Chunk index (0-based)
    pub index: u32,
    /// Offset in the full snapshot
    pub offset: u64,
    /// Chunk data
    pub data: Vec<u8>,
    /// Is this the last chunk?
    pub is_last: bool,
}

/// Snapshot writer - builds a snapshot incrementally
pub struct SnapshotWriter {
    space_id: u32,
    part_id: u32,
    store: Arc<dyn KVStore>,
    last_included_index: LogIndex,
    last_included_term: Term,
    data: Vec<u8>,
    chunk_size: usize,
}

impl SnapshotWriter {
    /// Create a new snapshot writer
    pub fn new(
        store: Arc<dyn KVStore>,
        space_id: u32,
        part_id: u32,
        last_included_index: LogIndex,
        last_included_term: Term,
    ) -> Self {
        Self {
            space_id,
            part_id,
            store,
            last_included_index,
            last_included_term,
            data: Vec::new(),
            chunk_size: 64 * 1024, // 64KB chunks
        }
    }

    /// Write data to the snapshot
    pub fn write(&mut self, data: &[u8]) {
        self.data.extend_from_slice(data);
    }

    /// Write a key-value pair to the snapshot
    pub fn write_kv(&mut self, key: &[u8], value: &[u8]) -> Result<(), SnapshotError> {
        // Format: [key_len: u32][key][value_len: u32][value]
        let key_len = key.len() as u32;
        let value_len = value.len() as u32;

        self.data.extend_from_slice(&key_len.to_le_bytes());
        self.data.extend_from_slice(key);
        self.data.extend_from_slice(&value_len.to_le_bytes());
        self.data.extend_from_slice(value);

        Ok(())
    }

    /// Finalize and save the snapshot
    pub async fn finish(self) -> Result<SnapshotMeta, SnapshotError> {
        let size = self.data.len() as u64;
        let chunk_count = size.div_ceil(self.chunk_size as u64) as u32;
        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let meta = SnapshotMeta {
            last_included_index: self.last_included_index,
            last_included_term: self.last_included_term,
            size,
            chunk_count,
            created_at,
        };

        // Save chunks
        for (i, chunk_data) in self.data.chunks(self.chunk_size).enumerate() {
            let chunk = SnapshotChunk {
                index: i as u32,
                offset: (i * self.chunk_size) as u64,
                data: chunk_data.to_vec(),
                is_last: i as u32 == chunk_count - 1,
            };

            let key = Self::chunk_key(self.space_id, self.part_id, i as u32);
            let value = bincode::serialize(&chunk)
                .map_err(|e| SnapshotError::Serialization(e.to_string()))?;

            self.store
                .put(&key, &value)
                .await
                .map_err(|e| SnapshotError::Storage(e.to_string()))?;
        }

        // Save metadata
        let meta_key = Self::meta_key(self.space_id, self.part_id);
        let meta_value =
            bincode::serialize(&meta).map_err(|e| SnapshotError::Serialization(e.to_string()))?;

        self.store
            .put(&meta_key, &meta_value)
            .await
            .map_err(|e| SnapshotError::Storage(e.to_string()))?;

        info!(
            "Created snapshot for space={}, part={}: index={}, term={}, size={} bytes, {} chunks",
            self.space_id,
            self.part_id,
            meta.last_included_index,
            meta.last_included_term,
            size,
            chunk_count
        );

        Ok(meta)
    }

    fn meta_key(space_id: u32, part_id: u32) -> Vec<u8> {
        let mut key = SNAPSHOT_META_PREFIX.to_vec();
        key.extend_from_slice(&space_id.to_be_bytes());
        key.extend_from_slice(&part_id.to_be_bytes());
        key
    }

    fn chunk_key(space_id: u32, part_id: u32, chunk_index: u32) -> Vec<u8> {
        let mut key = SNAPSHOT_DATA_PREFIX.to_vec();
        key.extend_from_slice(&space_id.to_be_bytes());
        key.extend_from_slice(&part_id.to_be_bytes());
        key.push(b':');
        key.extend_from_slice(&chunk_index.to_be_bytes());
        key
    }
}

/// Snapshot reader - reads a snapshot for sending or applying
pub struct SnapshotReader {
    space_id: u32,
    part_id: u32,
    store: Arc<dyn KVStore>,
}

impl SnapshotReader {
    /// Create a new snapshot reader
    pub fn new(store: Arc<dyn KVStore>, space_id: u32, part_id: u32) -> Self {
        Self {
            space_id,
            part_id,
            store,
        }
    }

    /// Load snapshot metadata
    pub async fn load_meta(&self) -> Result<Option<SnapshotMeta>, SnapshotError> {
        let key = SnapshotWriter::meta_key(self.space_id, self.part_id);
        match self.store.get(&key).await {
            Ok(Some(data)) => {
                let meta: SnapshotMeta = bincode::deserialize(&data)
                    .map_err(|e| SnapshotError::Deserialization(e.to_string()))?;
                Ok(Some(meta))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(SnapshotError::Storage(e.to_string())),
        }
    }

    /// Load a specific chunk
    pub async fn load_chunk(&self, index: u32) -> Result<Option<SnapshotChunk>, SnapshotError> {
        let key = SnapshotWriter::chunk_key(self.space_id, self.part_id, index);
        match self.store.get(&key).await {
            Ok(Some(data)) => {
                let chunk: SnapshotChunk = bincode::deserialize(&data)
                    .map_err(|e| SnapshotError::Deserialization(e.to_string()))?;
                Ok(Some(chunk))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(SnapshotError::Storage(e.to_string())),
        }
    }

    /// Load the complete snapshot data
    pub async fn load_all_data(&self) -> Result<Option<Vec<u8>>, SnapshotError> {
        let meta = match self.load_meta().await? {
            Some(m) => m,
            None => return Ok(None),
        };

        let mut data = Vec::with_capacity(meta.size as usize);
        for i in 0..meta.chunk_count {
            let chunk = self
                .load_chunk(i)
                .await?
                .ok_or_else(|| SnapshotError::Corruption(format!("Missing chunk {}", i)))?;
            data.extend_from_slice(&chunk.data);
        }

        Ok(Some(data))
    }

    /// Iterate over key-value pairs in the snapshot
    pub async fn iter_kvs(&self) -> Result<Vec<(Vec<u8>, Vec<u8>)>, SnapshotError> {
        let data = match self.load_all_data().await? {
            Some(d) => d,
            None => return Ok(Vec::new()),
        };

        let mut kvs = Vec::new();
        let mut offset = 0;

        while offset + 8 <= data.len() {
            // Read key length
            let key_len = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;

            if offset + key_len > data.len() {
                return Err(SnapshotError::Corruption("Truncated key".to_string()));
            }

            let key = data[offset..offset + key_len].to_vec();
            offset += key_len;

            // Read value length
            if offset + 4 > data.len() {
                return Err(SnapshotError::Corruption(
                    "Truncated value length".to_string(),
                ));
            }

            let value_len =
                u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;

            if offset + value_len > data.len() {
                return Err(SnapshotError::Corruption("Truncated value".to_string()));
            }

            let value = data[offset..offset + value_len].to_vec();
            offset += value_len;

            kvs.push((key, value));
        }

        Ok(kvs)
    }
}

/// Snapshot installer - receives and applies a snapshot from a leader
pub struct SnapshotInstaller {
    space_id: u32,
    part_id: u32,
    store: Arc<dyn KVStore>,
    pending_meta: Option<SnapshotMeta>,
    pending_chunks: Vec<Option<Vec<u8>>>,
    received_chunks: u32,
}

impl SnapshotInstaller {
    /// Create a new snapshot installer
    pub fn new(store: Arc<dyn KVStore>, space_id: u32, part_id: u32) -> Self {
        Self {
            space_id,
            part_id,
            store,
            pending_meta: None,
            pending_chunks: Vec::new(),
            received_chunks: 0,
        }
    }

    /// Start receiving a new snapshot
    pub fn begin(
        &mut self,
        last_included_index: LogIndex,
        last_included_term: Term,
        total_size: u64,
    ) {
        let chunk_size = 64 * 1024u64;
        let chunk_count = total_size.div_ceil(chunk_size) as u32;

        self.pending_meta = Some(SnapshotMeta {
            last_included_index,
            last_included_term,
            size: total_size,
            chunk_count,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        });

        self.pending_chunks = vec![None; chunk_count as usize];
        self.received_chunks = 0;

        debug!(
            "Started receiving snapshot: index={}, term={}, size={}, chunks={}",
            last_included_index, last_included_term, total_size, chunk_count
        );
    }

    /// Receive a chunk of snapshot data
    pub fn receive_chunk(
        &mut self,
        offset: u64,
        data: Vec<u8>,
        done: bool,
    ) -> Result<bool, SnapshotError> {
        let chunk_size = 64 * 1024usize;
        let chunk_index = (offset / chunk_size as u64) as usize;

        if chunk_index >= self.pending_chunks.len() {
            return Err(SnapshotError::InvalidChunk(format!(
                "Chunk index {} out of range",
                chunk_index
            )));
        }

        if self.pending_chunks[chunk_index].is_none() {
            self.pending_chunks[chunk_index] = Some(data);
            self.received_chunks += 1;
        }

        if done {
            // Verify we have all chunks
            let expected = self.pending_chunks.len() as u32;
            if self.received_chunks >= expected {
                return Ok(true); // Ready to install
            }
        }

        Ok(false)
    }

    /// Install the complete snapshot
    pub async fn install(&mut self) -> Result<SnapshotMeta, SnapshotError> {
        let meta = self
            .pending_meta
            .take()
            .ok_or_else(|| SnapshotError::InvalidState("No pending snapshot".to_string()))?;

        // Combine all chunks
        let mut data = Vec::with_capacity(meta.size as usize);
        for (i, chunk) in self.pending_chunks.drain(..).enumerate() {
            let chunk_data =
                chunk.ok_or_else(|| SnapshotError::InvalidChunk(format!("Missing chunk {}", i)))?;
            data.extend_from_slice(&chunk_data);
        }

        // Write snapshot using SnapshotWriter
        let mut writer = SnapshotWriter::new(
            Arc::clone(&self.store),
            self.space_id,
            self.part_id,
            meta.last_included_index,
            meta.last_included_term,
        );
        writer.write(&data);
        let saved_meta = writer.finish().await?;

        self.received_chunks = 0;

        info!(
            "Installed snapshot for space={}, part={}: index={}, term={}",
            self.space_id,
            self.part_id,
            saved_meta.last_included_index,
            saved_meta.last_included_term
        );

        Ok(saved_meta)
    }

    /// Cancel a pending snapshot installation
    pub fn cancel(&mut self) {
        self.pending_meta = None;
        self.pending_chunks.clear();
        self.received_chunks = 0;
    }
}

/// Errors from snapshot operations
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error("Storage error: {0}")]
    Storage(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Deserialization error: {0}")]
    Deserialization(String),

    #[error("Snapshot corruption: {0}")]
    Corruption(String),

    #[error("Invalid chunk: {0}")]
    InvalidChunk(String),

    #[error("Invalid state: {0}")]
    InvalidState(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use byoridb_kvstore::MemoryKVStore;

    #[tokio::test]
    async fn test_snapshot_write_and_read() {
        let store: Arc<dyn KVStore> = Arc::new(MemoryKVStore::new());

        // Write snapshot
        let mut writer = SnapshotWriter::new(
            Arc::clone(&store),
            1,
            1,  // space_id, part_id
            10, // last_included_index
            2,  // last_included_term
        );

        writer.write_kv(b"key1", b"value1").unwrap();
        writer.write_kv(b"key2", b"value2").unwrap();

        let meta = writer.finish().await.unwrap();
        assert_eq!(meta.last_included_index, 10);
        assert_eq!(meta.last_included_term, 2);

        // Read snapshot
        let reader = SnapshotReader::new(Arc::clone(&store), 1, 1);
        let loaded_meta = reader.load_meta().await.unwrap().unwrap();
        assert_eq!(loaded_meta.last_included_index, 10);

        let kvs = reader.iter_kvs().await.unwrap();
        assert_eq!(kvs.len(), 2);
        assert_eq!(kvs[0], (b"key1".to_vec(), b"value1".to_vec()));
        assert_eq!(kvs[1], (b"key2".to_vec(), b"value2".to_vec()));
    }

    #[tokio::test]
    async fn test_snapshot_installer() {
        let store: Arc<dyn KVStore> = Arc::new(MemoryKVStore::new());

        // Prepare test data
        let data = b"test snapshot data for installation".to_vec();

        let mut installer = SnapshotInstaller::new(Arc::clone(&store), 1, 1);

        // Begin installation
        installer.begin(15, 3, data.len() as u64);

        // Receive data (single chunk for small data)
        let ready = installer.receive_chunk(0, data.clone(), true).unwrap();
        assert!(ready);

        // Install
        let meta = installer.install().await.unwrap();
        assert_eq!(meta.last_included_index, 15);
        assert_eq!(meta.last_included_term, 3);

        // Verify snapshot was saved
        let reader = SnapshotReader::new(Arc::clone(&store), 1, 1);
        let loaded_meta = reader.load_meta().await.unwrap().unwrap();
        assert_eq!(loaded_meta.last_included_index, 15);
    }

    #[tokio::test]
    async fn test_no_snapshot() {
        let store: Arc<dyn KVStore> = Arc::new(MemoryKVStore::new());
        let reader = SnapshotReader::new(store, 1, 1);

        let meta = reader.load_meta().await.unwrap();
        assert!(meta.is_none());
    }
}

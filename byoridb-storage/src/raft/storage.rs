// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Raft persistent storage
//!
//! This module provides persistent storage for Raft state that must survive crashes:
//! - Current term
//! - Voted for (in current term)
//! - Log entries
//!
//! Uses the KVStore backend for durability.

use super::types::{LogEntry, LogIndex, NodeId, Term};
use byoridb_kvstore::KVStore;

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, info};

/// Key prefixes for Raft persistent state
const STATE_PREFIX: &[u8] = b"raft:state:";
const LOG_PREFIX: &[u8] = b"raft:log:";

/// Persistent state that must be saved before responding to RPCs
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RaftPersistentState {
    /// Latest term server has seen
    pub current_term: Term,
    /// CandidateId that received vote in current term (or None)
    pub voted_for: Option<NodeId>,
}

/// Storage backend for Raft persistent state
pub struct RaftStorage {
    /// KVStore backend
    store: Arc<dyn KVStore>,
    /// Space ID for this Raft group
    space_id: u32,
    /// Partition ID for this Raft group
    part_id: u32,
}

impl RaftStorage {
    /// Create a new RaftStorage instance
    pub fn new(store: Arc<dyn KVStore>, space_id: u32, part_id: u32) -> Self {
        Self {
            store,
            space_id,
            part_id,
        }
    }

    /// Generate the key for persistent state
    fn state_key(&self) -> Vec<u8> {
        let mut key = STATE_PREFIX.to_vec();
        key.extend_from_slice(&self.space_id.to_be_bytes());
        key.extend_from_slice(&self.part_id.to_be_bytes());
        key
    }

    /// Generate the key for a log entry
    fn log_key(&self, index: LogIndex) -> Vec<u8> {
        let mut key = LOG_PREFIX.to_vec();
        key.extend_from_slice(&self.space_id.to_be_bytes());
        key.extend_from_slice(&self.part_id.to_be_bytes());
        key.push(b':');
        key.extend_from_slice(&index.to_be_bytes());
        key
    }

    /// Generate the key prefix for log entries
    fn log_prefix(&self) -> Vec<u8> {
        let mut key = LOG_PREFIX.to_vec();
        key.extend_from_slice(&self.space_id.to_be_bytes());
        key.extend_from_slice(&self.part_id.to_be_bytes());
        key.push(b':');
        key
    }

    /// Load persistent state from storage
    pub async fn load_state(&self) -> Result<RaftPersistentState, RaftStorageError> {
        let key = self.state_key();
        match self.store.get(&key).await {
            Ok(Some(data)) => {
                let state: RaftPersistentState = bincode::deserialize(&data)
                    .map_err(|e| RaftStorageError::Deserialization(e.to_string()))?;
                debug!(
                    "Loaded Raft state for space={}, part={}: term={}, voted_for={:?}",
                    self.space_id, self.part_id, state.current_term, state.voted_for
                );
                Ok(state)
            }
            Ok(None) => {
                debug!(
                    "No persisted Raft state for space={}, part={}, using default",
                    self.space_id, self.part_id
                );
                Ok(RaftPersistentState::default())
            }
            Err(e) => Err(RaftStorageError::Store(e.to_string())),
        }
    }

    /// Save persistent state to storage
    pub async fn save_state(&self, state: &RaftPersistentState) -> Result<(), RaftStorageError> {
        let key = self.state_key();
        let data = bincode::serialize(state)
            .map_err(|e| RaftStorageError::Serialization(e.to_string()))?;

        self.store
            .put(&key, &data)
            .await
            .map_err(|e| RaftStorageError::Store(e.to_string()))?;

        debug!(
            "Saved Raft state for space={}, part={}: term={}, voted_for={:?}",
            self.space_id, self.part_id, state.current_term, state.voted_for
        );
        Ok(())
    }

    /// Load a log entry from storage
    pub async fn load_entry(&self, index: LogIndex) -> Result<Option<LogEntry>, RaftStorageError> {
        let key = self.log_key(index);
        match self.store.get(&key).await {
            Ok(Some(data)) => {
                let entry: LogEntry = bincode::deserialize(&data)
                    .map_err(|e| RaftStorageError::Deserialization(e.to_string()))?;
                Ok(Some(entry))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(RaftStorageError::Store(e.to_string())),
        }
    }

    /// Save a log entry to storage
    pub async fn save_entry(&self, entry: &LogEntry) -> Result<(), RaftStorageError> {
        let key = self.log_key(entry.index);
        let data = bincode::serialize(entry)
            .map_err(|e| RaftStorageError::Serialization(e.to_string()))?;

        self.store
            .put(&key, &data)
            .await
            .map_err(|e| RaftStorageError::Store(e.to_string()))?;

        Ok(())
    }

    /// Save multiple log entries to storage
    pub async fn save_entries(&self, entries: &[LogEntry]) -> Result<(), RaftStorageError> {
        for entry in entries {
            self.save_entry(entry).await?;
        }
        Ok(())
    }

    /// Delete log entries starting from the given index (inclusive)
    pub async fn delete_entries_from(&self, start_index: LogIndex) -> Result<(), RaftStorageError> {
        let prefix = self.log_prefix();
        let entries = self
            .store
            .scan_prefix(&prefix)
            .await
            .map_err(|e| RaftStorageError::Store(e.to_string()))?;

        for (key, _) in entries {
            // Parse the index from the key
            if key.len() >= prefix.len() + 8 {
                let index_bytes = &key[prefix.len()..prefix.len() + 8];
                if let Ok(bytes) = index_bytes.try_into() {
                    let index = u64::from_be_bytes(bytes);
                    if index >= start_index {
                        self.store
                            .delete(&key)
                            .await
                            .map_err(|e| RaftStorageError::Store(e.to_string()))?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Load all log entries from storage
    pub async fn load_all_entries(&self) -> Result<Vec<LogEntry>, RaftStorageError> {
        let prefix = self.log_prefix();
        let entries = self
            .store
            .scan_prefix(&prefix)
            .await
            .map_err(|e| RaftStorageError::Store(e.to_string()))?;

        let mut log_entries = Vec::new();
        for (_, data) in entries {
            let entry: LogEntry = bincode::deserialize(&data)
                .map_err(|e| RaftStorageError::Deserialization(e.to_string()))?;
            log_entries.push(entry);
        }

        // Sort by index
        log_entries.sort_by_key(|e| e.index);
        Ok(log_entries)
    }

    /// Delete all entries with index less than the given index (for log compaction)
    pub async fn compact_log(&self, up_to_index: LogIndex) -> Result<usize, RaftStorageError> {
        let prefix = self.log_prefix();
        let entries = self
            .store
            .scan_prefix(&prefix)
            .await
            .map_err(|e| RaftStorageError::Store(e.to_string()))?;

        let mut deleted = 0;
        for (key, _) in entries {
            // Parse the index from the key
            if key.len() >= prefix.len() + 8 {
                let index_bytes = &key[prefix.len()..prefix.len() + 8];
                if let Ok(bytes) = index_bytes.try_into() {
                    let index = u64::from_be_bytes(bytes);
                    if index < up_to_index {
                        self.store
                            .delete(&key)
                            .await
                            .map_err(|e| RaftStorageError::Store(e.to_string()))?;
                        deleted += 1;
                    }
                }
            }
        }

        info!(
            "Compacted {} log entries for space={}, part={}",
            deleted, self.space_id, self.part_id
        );
        Ok(deleted)
    }
}

/// Errors from Raft storage operations
#[derive(Debug, thiserror::Error)]
pub enum RaftStorageError {
    #[error("Storage error: {0}")]
    Store(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Deserialization error: {0}")]
    Deserialization(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raft::Command;
    use byoridb_kvstore::MemoryKVStore;

    #[tokio::test]
    async fn test_save_and_load_state() {
        let store = Arc::new(MemoryKVStore::new());
        let storage = RaftStorage::new(store, 1, 1);

        let state = RaftPersistentState {
            current_term: 5,
            voted_for: Some(2),
        };

        storage.save_state(&state).await.unwrap();
        let loaded = storage.load_state().await.unwrap();

        assert_eq!(loaded.current_term, 5);
        assert_eq!(loaded.voted_for, Some(2));
    }

    #[tokio::test]
    async fn test_load_empty_state() {
        let store = Arc::new(MemoryKVStore::new());
        let storage = RaftStorage::new(store, 1, 1);

        let state = storage.load_state().await.unwrap();
        assert_eq!(state.current_term, 0);
        assert_eq!(state.voted_for, None);
    }

    #[tokio::test]
    async fn test_save_and_load_entries() {
        let store = Arc::new(MemoryKVStore::new());
        let storage = RaftStorage::new(store, 1, 1);

        let entries = vec![
            LogEntry {
                term: 1,
                index: 1,
                command: Command::Noop,
            },
            LogEntry {
                term: 1,
                index: 2,
                command: Command::Noop,
            },
            LogEntry {
                term: 2,
                index: 3,
                command: Command::Noop,
            },
        ];

        storage.save_entries(&entries).await.unwrap();
        let loaded = storage.load_all_entries().await.unwrap();

        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded[0].index, 1);
        assert_eq!(loaded[1].index, 2);
        assert_eq!(loaded[2].index, 3);
    }

    #[tokio::test]
    async fn test_delete_entries_from() {
        let store = Arc::new(MemoryKVStore::new());
        let storage = RaftStorage::new(store, 1, 1);

        let entries = vec![
            LogEntry {
                term: 1,
                index: 1,
                command: Command::Noop,
            },
            LogEntry {
                term: 1,
                index: 2,
                command: Command::Noop,
            },
            LogEntry {
                term: 2,
                index: 3,
                command: Command::Noop,
            },
        ];

        storage.save_entries(&entries).await.unwrap();
        storage.delete_entries_from(2).await.unwrap();

        let loaded = storage.load_all_entries().await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].index, 1);
    }

    #[tokio::test]
    async fn test_compact_log() {
        let store = Arc::new(MemoryKVStore::new());
        let storage = RaftStorage::new(store, 1, 1);

        let entries = vec![
            LogEntry {
                term: 1,
                index: 1,
                command: Command::Noop,
            },
            LogEntry {
                term: 1,
                index: 2,
                command: Command::Noop,
            },
            LogEntry {
                term: 2,
                index: 3,
                command: Command::Noop,
            },
        ];

        storage.save_entries(&entries).await.unwrap();
        let deleted = storage.compact_log(3).await.unwrap();

        assert_eq!(deleted, 2);

        let loaded = storage.load_all_entries().await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].index, 3);
    }
}

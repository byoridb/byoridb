// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

pub mod backup;
pub mod error;
pub mod store;

pub use backup::{
    format_bytes, format_timestamp, BackupError, BackupInfo, BackupManager, BackupOptions,
};
pub use error::{KVStoreError, Result};
// Pure-Rust KV store: the trait, the redb-backed and in-memory backends, and
// the scan filter types.
pub use store::{FilterFn, KVStore, KVStoreOptions, MemoryKVStore, RedbKVStore};

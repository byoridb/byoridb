// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

// backup uses RocksDB's Checkpoint feature — only available with `rocksdb`.
#[cfg(feature = "rocksdb")]
pub mod backup;
pub mod error;
pub mod store;
pub mod wal;

#[cfg(feature = "rocksdb")]
pub use backup::{
    format_bytes, format_timestamp, BackupError, BackupInfo, BackupManager, BackupOptions,
};
pub use error::{KVStoreError, Result};
// Always available (pure Rust): the KVStore trait, the in-memory backend, and
// the scan filter type. RocksDB-backed types require the `rocksdb` feature.
pub use store::{FilterFn, KVStore, MemoryKVStore};
#[cfg(feature = "rocksdb")]
pub use store::{KVStoreOptions, RocksdbKVStore, WalKVStore};
pub use wal::{OpType, WalEntry, WAL};

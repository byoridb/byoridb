// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use crate::error::Result;
#[cfg(feature = "rocksdb")]
use crate::wal::{OpType, WAL};
use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
#[cfg(feature = "rocksdb")]
use rocksdb::{
    BlockBasedOptions, Cache, Direction, IteratorMode, Options, WriteBatch as RocksDBWriteBatch,
    DB as RocksDBDB,
};
#[cfg(feature = "rocksdb")]
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(feature = "rocksdb")]
use tokio_stream::wrappers::ReceiverStream;
#[cfg(feature = "rocksdb")]
use tracing::info;

/// Item yielded by [`KVStore::scan_stream`]. Each `(key, value)` is owned so
/// the stream can outlive the underlying iterator.
pub type ScanStreamItem = Result<(Vec<u8>, Vec<u8>)>;

#[cfg(feature = "rocksdb")]
#[derive(Debug, Clone)]
pub struct KVStoreOptions {
    pub create_if_missing: bool,
    pub max_open_files: i32,
    pub use_fsync: bool,
    pub compression: rocksdb::DBCompressionType,
    pub wal_enabled: bool,
    pub wal_dir: Option<PathBuf>,
    pub wal_max_file_size: u64,
    /// Bloom filter bits per key (0 to disable, 10 recommended)
    pub bloom_filter_bits: i32,
    /// Block cache size in bytes (256MB default)
    pub block_cache_size: usize,
    /// RocksDB write buffer (memtable) size in bytes (64MB default).
    /// Total memtable memory ≈ write_buffer_size × max_write_buffer_number.
    pub write_buffer_size: usize,
    /// Maximum number of write buffers (memtables) before stalling writes.
    pub max_write_buffer_number: i32,
}

#[cfg(feature = "rocksdb")]
impl Default for KVStoreOptions {
    fn default() -> Self {
        KVStoreOptions {
            create_if_missing: true,
            max_open_files: 5000,
            use_fsync: false,
            compression: rocksdb::DBCompressionType::Lz4,
            wal_enabled: true,
            wal_dir: None,
            wal_max_file_size: 64 * 1024 * 1024, // 64MB
            bloom_filter_bits: 10,               // 10 bits per key for ~1% false positive
            block_cache_size: 256 * 1024 * 1024, // 256MB block cache
            write_buffer_size: 64 * 1024 * 1024, // 64MB per memtable
            max_write_buffer_number: 3,          // up to 3 memtables → max ~192MB
        }
    }
}

/// Filter function type for scan_with_filter
/// Takes (key, value) and returns true if the row should be included
pub type FilterFn = Box<dyn Fn(&[u8], &[u8]) -> bool + Send + Sync>;

/// Visitor function type for streaming scans.
/// Return true to continue scanning, false to stop early.
pub type ScanVisitorFn = Box<dyn FnMut(&[u8], &[u8]) -> bool + Send>;

#[async_trait]
pub trait KVStore: Send + Sync {
    async fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>>;
    async fn put(&self, key: &[u8], value: &[u8]) -> Result<()>;
    async fn delete(&self, key: &[u8]) -> Result<()>;
    async fn batch_put(&self, pairs: Vec<(Vec<u8>, Vec<u8>)>) -> Result<()>;
    async fn scan_prefix(&self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>>;

    /// Batch get multiple keys in a single operation
    /// Returns results in the same order as input keys
    async fn batch_get(&self, keys: &[Vec<u8>]) -> Result<Vec<Option<Vec<u8>>>>;

    /// Scan with predicate pushdown - filter is evaluated at storage layer
    /// This reduces network I/O by only returning rows that pass the filter.
    /// The filter function receives (key, value) and returns true to include the row.
    ///
    /// Default implementation falls back to scan_prefix + in-memory filtering.
    async fn scan_with_filter(
        &self,
        prefix: &[u8],
        filter: FilterFn,
        limit: Option<usize>,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let results = self.scan_prefix(prefix).await?;
        let filtered: Vec<_> = results
            .into_iter()
            .filter(|(k, v)| filter(k, v))
            .take(limit.unwrap_or(usize::MAX))
            .collect();
        Ok(filtered)
    }

    /// Prefix scan with an optional result cap.
    ///
    /// Implementations that override [`scan_with_filter`] can stop storage
    /// iteration once the limit is reached, avoiding full prefix materialization.
    async fn scan_prefix_limited(
        &self,
        prefix: &[u8],
        limit: Option<usize>,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        self.scan_with_filter(prefix, Box::new(|_, _| true), limit)
            .await
    }

    /// Scan with predicate pushdown and visit matching rows without forcing
    /// the storage implementation to return a materialized Vec.
    ///
    /// The visitor returns false to stop early. The returned usize is the
    /// number of rows passed to the visitor.
    async fn scan_with_filter_visit(
        &self,
        prefix: &[u8],
        filter: FilterFn,
        mut visitor: ScanVisitorFn,
        limit: Option<usize>,
    ) -> Result<usize> {
        let results = self.scan_with_filter(prefix, filter, limit).await?;
        let mut visited = 0;
        for (key, value) in results {
            visited += 1;
            if !visitor(&key, &value) {
                break;
            }
        }
        Ok(visited)
    }

    /// Stream a prefix scan as `(key, value)` items without materializing the
    /// entire range into memory.
    ///
    /// This is the canonical "real" streaming API: callers drive iteration
    /// with `.next().await` and can stop at any point — the underlying
    /// storage iterator is dropped as soon as the stream is dropped, so an
    /// early-terminating consumer (BFS finding a target, MATCH hitting a
    /// limit) pays only for what it actually consumed.
    ///
    /// Default implementation calls [`Self::scan_prefix`] and wraps the
    /// returned Vec in a stream. Production-grade backends override this
    /// to use a channel-driven iterator so high-degree vertex traversals
    /// don't `Vec`-materialize all neighbors up front.
    async fn scan_stream(&self, prefix: &[u8]) -> Result<BoxStream<'static, ScanStreamItem>> {
        let results = self.scan_prefix(prefix).await?;
        Ok(futures::stream::iter(results.into_iter().map(Ok)).boxed())
    }
}

#[cfg(feature = "rocksdb")]
pub struct RocksdbKVStore {
    db: Arc<RocksDBDB>,
}

#[cfg(feature = "rocksdb")]
impl RocksdbKVStore {
    pub fn open<P: AsRef<Path>>(path: P, opts: KVStoreOptions) -> Result<Self> {
        let mut db_opts = Options::default();
        db_opts.create_if_missing(opts.create_if_missing);
        db_opts.set_max_open_files(opts.max_open_files);
        db_opts.set_use_fsync(opts.use_fsync);
        db_opts.set_compression_type(opts.compression);

        // Memtable memory cap: write_buffer_size × max_write_buffer_number
        if opts.write_buffer_size > 0 {
            db_opts.set_write_buffer_size(opts.write_buffer_size);
        }
        if opts.max_write_buffer_number > 0 {
            db_opts.set_max_write_buffer_number(opts.max_write_buffer_number);
        }

        // Configure block-based table with Bloom filter and block cache
        let mut block_opts = BlockBasedOptions::default();
        if opts.bloom_filter_bits > 0 {
            block_opts.set_bloom_filter(opts.bloom_filter_bits as f64, false);
        }
        if opts.block_cache_size > 0 {
            let cache = Cache::new_lru_cache(opts.block_cache_size);
            block_opts.set_block_cache(&cache);
        }
        db_opts.set_block_based_table_factory(&block_opts);

        let db = RocksDBDB::open(&db_opts, path)?;
        info!(
            "RocksDB opened with bloom_filter_bits={}, block_cache_size={}MB",
            opts.bloom_filter_bits,
            opts.block_cache_size / (1024 * 1024)
        );
        Ok(RocksdbKVStore { db: Arc::new(db) })
    }

    pub fn with_db(db: Arc<RocksDBDB>) -> Self {
        RocksdbKVStore { db }
    }
}

#[cfg(feature = "rocksdb")]
#[async_trait]
impl KVStore for RocksdbKVStore {
    /// Single key get - uses block_in_place for lower overhead
    async fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let db = Arc::clone(&self.db);
        let key = key.to_vec();

        // Use block_in_place for single operations (lower overhead than spawn_blocking)
        tokio::task::block_in_place(move || Ok(db.get(&key)?))
    }

    /// Single key put - uses block_in_place for lower overhead
    async fn put(&self, key: &[u8], value: &[u8]) -> Result<()> {
        let db = Arc::clone(&self.db);
        let key = key.to_vec();
        let value = value.to_vec();

        tokio::task::block_in_place(move || Ok(db.put(&key, &value)?))
    }

    /// Single key delete - uses block_in_place for lower overhead
    async fn delete(&self, key: &[u8]) -> Result<()> {
        let db = Arc::clone(&self.db);
        let key = key.to_vec();

        tokio::task::block_in_place(move || Ok(db.delete(&key)?))
    }

    /// Batch put - uses spawn_blocking for longer-running operations
    async fn batch_put(&self, pairs: Vec<(Vec<u8>, Vec<u8>)>) -> Result<()> {
        let db = Arc::clone(&self.db);

        tokio::task::spawn_blocking(move || {
            let mut batch = RocksDBWriteBatch::default();
            for (key, value) in pairs {
                batch.put(&key, &value);
            }
            Ok(db.write(batch)?)
        })
        .await?
    }

    /// Prefix scan - uses spawn_blocking as it may iterate over many keys
    async fn scan_prefix(&self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let db = Arc::clone(&self.db);
        let prefix = prefix.to_vec();

        tokio::task::spawn_blocking(move || {
            let mut results = Vec::new();
            let iter = db.iterator(IteratorMode::From(&prefix, Direction::Forward));
            for item in iter {
                let (key, value) = item?;
                if !key.starts_with(&prefix) {
                    break;
                }
                results.push((key.to_vec(), value.to_vec()));
            }
            Ok(results)
        })
        .await?
    }

    /// Batch get - uses spawn_blocking for potentially large batches
    async fn batch_get(&self, keys: &[Vec<u8>]) -> Result<Vec<Option<Vec<u8>>>> {
        let db = Arc::clone(&self.db);
        let keys = keys.to_vec();

        tokio::task::spawn_blocking(move || {
            let key_refs: Vec<&[u8]> = keys.iter().map(|k| k.as_slice()).collect();
            let results = db.multi_get(&key_refs);
            let mut output = Vec::with_capacity(results.len());
            for result in results {
                match result {
                    Ok(opt) => output.push(opt),
                    Err(e) => return Err(e.into()),
                }
            }
            Ok(output)
        })
        .await?
    }

    /// Optimized scan with filter - applies filter during iteration
    /// This avoids collecting all results before filtering
    async fn scan_with_filter(
        &self,
        prefix: &[u8],
        filter: FilterFn,
        limit: Option<usize>,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let db = Arc::clone(&self.db);
        let prefix = prefix.to_vec();
        let limit = limit.unwrap_or(usize::MAX);
        if limit == 0 {
            return Ok(Vec::new());
        }

        tokio::task::spawn_blocking(move || {
            let mut results = Vec::new();
            let iter = db.iterator(IteratorMode::From(&prefix, Direction::Forward));

            for item in iter {
                let (key, value) = item?;
                if !key.starts_with(&prefix) {
                    break;
                }

                // Apply filter during iteration (predicate pushdown)
                if filter(&key, &value) {
                    results.push((key.to_vec(), value.to_vec()));

                    // Early exit if limit reached
                    if results.len() >= limit {
                        break;
                    }
                }
            }
            Ok(results)
        })
        .await?
    }

    async fn scan_with_filter_visit(
        &self,
        prefix: &[u8],
        filter: FilterFn,
        mut visitor: ScanVisitorFn,
        limit: Option<usize>,
    ) -> Result<usize> {
        let db = Arc::clone(&self.db);
        let prefix = prefix.to_vec();
        let limit = limit.unwrap_or(usize::MAX);
        if limit == 0 {
            return Ok(0);
        }

        tokio::task::spawn_blocking(move || {
            let mut visited = 0;
            let iter = db.iterator(IteratorMode::From(&prefix, Direction::Forward));

            for item in iter {
                let (key, value) = item?;
                if !key.starts_with(&prefix) {
                    break;
                }
                if filter(&key, &value) {
                    visited += 1;
                    if !visitor(&key, &value) || visited >= limit {
                        break;
                    }
                }
            }

            Ok(visited)
        })
        .await?
    }

    /// Channel-driven streaming scan for RocksDB.
    ///
    /// A `spawn_blocking` task owns the prefix iterator and forwards rows
    /// over a bounded mpsc channel. When the consumer drops the stream the
    /// channel sender errors, the task exits, and the iterator is dropped —
    /// partial consumption never materializes the entire range.
    async fn scan_stream(&self, prefix: &[u8]) -> Result<BoxStream<'static, ScanStreamItem>> {
        let db = Arc::clone(&self.db);
        let prefix = prefix.to_vec();
        let (tx, rx) = tokio::sync::mpsc::channel::<ScanStreamItem>(64);
        tokio::task::spawn_blocking(move || {
            let iter = db.iterator(IteratorMode::From(&prefix, Direction::Forward));
            for item in iter {
                let send_result = match item {
                    Ok((k, v)) => {
                        if !k.starts_with(&prefix) {
                            break;
                        }
                        tx.blocking_send(Ok((k.to_vec(), v.to_vec())))
                    }
                    Err(e) => tx.blocking_send(Err(e.into())),
                };
                if send_result.is_err() {
                    break;
                }
            }
        });
        Ok(ReceiverStream::new(rx).boxed())
    }
}

/// In-memory KVStore for testing
pub struct MemoryKVStore {
    data: Arc<tokio::sync::RwLock<std::collections::BTreeMap<Vec<u8>, Vec<u8>>>>,
}

impl MemoryKVStore {
    pub fn new() -> Self {
        MemoryKVStore {
            data: Arc::new(tokio::sync::RwLock::new(std::collections::BTreeMap::new())),
        }
    }
}

impl Default for MemoryKVStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl KVStore for MemoryKVStore {
    async fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let data = self.data.read().await;
        Ok(data.get(key).cloned())
    }

    async fn put(&self, key: &[u8], value: &[u8]) -> Result<()> {
        let mut data = self.data.write().await;
        data.insert(key.to_vec(), value.to_vec());
        Ok(())
    }

    async fn delete(&self, key: &[u8]) -> Result<()> {
        let mut data = self.data.write().await;
        data.remove(key);
        Ok(())
    }

    async fn batch_put(&self, pairs: Vec<(Vec<u8>, Vec<u8>)>) -> Result<()> {
        let mut data = self.data.write().await;
        for (key, value) in pairs {
            data.insert(key, value);
        }
        Ok(())
    }

    async fn scan_prefix(&self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let data = self.data.read().await;
        let results: Vec<_> = data
            .range(prefix.to_vec()..)
            .take_while(|(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        Ok(results)
    }

    async fn batch_get(&self, keys: &[Vec<u8>]) -> Result<Vec<Option<Vec<u8>>>> {
        let data = self.data.read().await;
        let results: Vec<_> = keys.iter().map(|k| data.get(k).cloned()).collect();
        Ok(results)
    }

    async fn scan_with_filter(
        &self,
        prefix: &[u8],
        filter: FilterFn,
        limit: Option<usize>,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let data = self.data.read().await;
        let limit = limit.unwrap_or(usize::MAX);

        let results: Vec<_> = data
            .range(prefix.to_vec()..)
            .take_while(|(k, _)| k.starts_with(prefix))
            .filter(|(k, v)| filter(k, v))
            .take(limit)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        Ok(results)
    }

    async fn scan_with_filter_visit(
        &self,
        prefix: &[u8],
        filter: FilterFn,
        mut visitor: ScanVisitorFn,
        limit: Option<usize>,
    ) -> Result<usize> {
        let data = self.data.read().await;
        let limit = limit.unwrap_or(usize::MAX);
        if limit == 0 {
            return Ok(0);
        }

        let mut visited = 0;
        for (key, value) in data
            .range(prefix.to_vec()..)
            .take_while(|(k, _)| k.starts_with(prefix))
        {
            if filter(key, value) {
                visited += 1;
                if !visitor(key, value) || visited >= limit {
                    break;
                }
            }
        }

        Ok(visited)
    }

    /// Streaming scan for Memory. Snapshots the matching range under the
    /// read lock and yields from the snapshot. This keeps lock hold time
    /// bounded while preserving the streaming `next().await` contract.
    /// Memory is the test backend so a snapshot-Vec is acceptable; RocksDB
    /// is the production path and has true incremental streaming.
    async fn scan_stream(&self, prefix: &[u8]) -> Result<BoxStream<'static, ScanStreamItem>> {
        let data = self.data.read().await;
        let snapshot: Vec<(Vec<u8>, Vec<u8>)> = data
            .range(prefix.to_vec()..)
            .take_while(|(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        drop(data);
        Ok(futures::stream::iter(snapshot.into_iter().map(Ok)).boxed())
    }
}

/// WAL-enabled KVStore wrapper
/// Writes to WAL before applying to RocksDB for durability
#[cfg(feature = "rocksdb")]
pub struct WalKVStore {
    db: Arc<RocksDBDB>,
    wal: Arc<WAL>,
}

#[cfg(feature = "rocksdb")]
impl WalKVStore {
    /// Open a WAL-enabled KVStore
    pub fn open<P: AsRef<Path>>(path: P, opts: KVStoreOptions) -> Result<Self> {
        let path = path.as_ref();

        // Setup RocksDB options
        let mut db_opts = Options::default();
        db_opts.create_if_missing(opts.create_if_missing);
        db_opts.set_max_open_files(opts.max_open_files);
        db_opts.set_use_fsync(opts.use_fsync);
        db_opts.set_compression_type(opts.compression);

        // Configure block-based table with Bloom filter and block cache
        let mut block_opts = BlockBasedOptions::default();
        if opts.bloom_filter_bits > 0 {
            block_opts.set_bloom_filter(opts.bloom_filter_bits as f64, false);
        }
        if opts.block_cache_size > 0 {
            let cache = Cache::new_lru_cache(opts.block_cache_size);
            block_opts.set_block_cache(&cache);
        }
        db_opts.set_block_based_table_factory(&block_opts);

        // Open RocksDB
        let db = Arc::new(RocksDBDB::open(&db_opts, path)?);

        // Setup WAL directory
        let wal_dir = opts.wal_dir.unwrap_or_else(|| path.join("wal"));
        let wal = Arc::new(WAL::new(&wal_dir, opts.wal_max_file_size)?);

        // Recover from WAL if needed
        let store = WalKVStore { db, wal };
        store.recover()?;

        info!(
            "WalKVStore opened at {:?} with WAL at {:?}, bloom_filter_bits={}, block_cache_size={}MB",
            path, wal_dir, opts.bloom_filter_bits, opts.block_cache_size / (1024 * 1024)
        );

        Ok(store)
    }

    /// Recover from WAL - replay all logged operations
    fn recover(&self) -> Result<()> {
        let entries = self.wal.recover()?;

        if entries.is_empty() {
            return Ok(());
        }

        info!("Recovering {} WAL entries", entries.len());

        for entry in entries {
            match entry.op {
                OpType::Put => {
                    self.db.put(&entry.key, &entry.value)?;
                }
                OpType::Delete => {
                    self.db.delete(&entry.key)?;
                }
            }
        }

        info!("WAL recovery complete");
        Ok(())
    }

    /// Sync WAL to disk
    pub fn sync(&self) -> Result<()> {
        self.wal.sync()
    }

    /// Cleanup old WAL files
    pub fn cleanup_wal(&self, min_lsn: u64) -> Result<()> {
        self.wal.cleanup(min_lsn)
    }

    /// Get current WAL LSN
    pub fn current_lsn(&self) -> u64 {
        self.wal.current_lsn()
    }
}

#[cfg(feature = "rocksdb")]
#[async_trait]
impl KVStore for WalKVStore {
    /// Single key get - uses block_in_place for lower overhead
    async fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let db = Arc::clone(&self.db);
        let key = key.to_vec();

        tokio::task::block_in_place(move || Ok(db.get(&key)?))
    }

    /// Single key put with WAL - uses block_in_place for lower overhead
    async fn put(&self, key: &[u8], value: &[u8]) -> Result<()> {
        // Write to WAL first
        self.wal.append_put(key, value)?;

        // Then write to RocksDB
        let db = Arc::clone(&self.db);
        let key = key.to_vec();
        let value = value.to_vec();

        tokio::task::block_in_place(move || Ok(db.put(&key, &value)?))
    }

    /// Single key delete with WAL - uses block_in_place for lower overhead
    async fn delete(&self, key: &[u8]) -> Result<()> {
        // Write to WAL first
        self.wal.append_delete(key)?;

        // Then delete from RocksDB
        let db = Arc::clone(&self.db);
        let key = key.to_vec();

        tokio::task::block_in_place(move || Ok(db.delete(&key)?))
    }

    /// Batch put with WAL - uses spawn_blocking for longer-running operations
    async fn batch_put(&self, pairs: Vec<(Vec<u8>, Vec<u8>)>) -> Result<()> {
        // Convert to WAL batch format and write all at once (single flush)
        let wal_entries: Vec<_> = pairs
            .iter()
            .map(|(k, v)| (OpType::Put, k.clone(), v.clone()))
            .collect();
        self.wal.append_batch(&wal_entries)?;

        // Then batch write to RocksDB
        let db = Arc::clone(&self.db);

        tokio::task::spawn_blocking(move || {
            let mut batch = RocksDBWriteBatch::default();
            for (key, value) in pairs {
                batch.put(&key, &value);
            }
            Ok(db.write(batch)?)
        })
        .await?
    }

    /// Prefix scan - uses spawn_blocking as it may iterate over many keys
    async fn scan_prefix(&self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let db = Arc::clone(&self.db);
        let prefix = prefix.to_vec();

        tokio::task::spawn_blocking(move || {
            let mut results = Vec::new();
            let iter = db.iterator(IteratorMode::From(&prefix, Direction::Forward));
            for item in iter {
                let (key, value) = item?;
                if !key.starts_with(&prefix) {
                    break;
                }
                results.push((key.to_vec(), value.to_vec()));
            }
            Ok(results)
        })
        .await?
    }

    /// Batch get - uses spawn_blocking for potentially large batches
    async fn batch_get(&self, keys: &[Vec<u8>]) -> Result<Vec<Option<Vec<u8>>>> {
        let db = Arc::clone(&self.db);
        let keys = keys.to_vec();

        tokio::task::spawn_blocking(move || {
            let key_refs: Vec<&[u8]> = keys.iter().map(|k| k.as_slice()).collect();
            let results = db.multi_get(&key_refs);
            let mut output = Vec::with_capacity(results.len());
            for result in results {
                match result {
                    Ok(opt) => output.push(opt),
                    Err(e) => return Err(e.into()),
                }
            }
            Ok(output)
        })
        .await?
    }

    /// Optimized scan with filter - applies filter during iteration
    async fn scan_with_filter(
        &self,
        prefix: &[u8],
        filter: FilterFn,
        limit: Option<usize>,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let db = Arc::clone(&self.db);
        let prefix = prefix.to_vec();
        let limit = limit.unwrap_or(usize::MAX);
        if limit == 0 {
            return Ok(Vec::new());
        }

        tokio::task::spawn_blocking(move || {
            let mut results = Vec::new();
            let iter = db.iterator(IteratorMode::From(&prefix, Direction::Forward));

            for item in iter {
                let (key, value) = item?;
                if !key.starts_with(&prefix) {
                    break;
                }

                // Apply filter during iteration (predicate pushdown)
                if filter(&key, &value) {
                    results.push((key.to_vec(), value.to_vec()));

                    // Early exit if limit reached
                    if results.len() >= limit {
                        break;
                    }
                }
            }
            Ok(results)
        })
        .await?
    }

    async fn scan_with_filter_visit(
        &self,
        prefix: &[u8],
        filter: FilterFn,
        mut visitor: ScanVisitorFn,
        limit: Option<usize>,
    ) -> Result<usize> {
        let db = Arc::clone(&self.db);
        let prefix = prefix.to_vec();
        let limit = limit.unwrap_or(usize::MAX);
        if limit == 0 {
            return Ok(0);
        }

        tokio::task::spawn_blocking(move || {
            let mut visited = 0;
            let iter = db.iterator(IteratorMode::From(&prefix, Direction::Forward));

            for item in iter {
                let (key, value) = item?;
                if !key.starts_with(&prefix) {
                    break;
                }
                if filter(&key, &value) {
                    visited += 1;
                    if !visitor(&key, &value) || visited >= limit {
                        break;
                    }
                }
            }

            Ok(visited)
        })
        .await?
    }

    /// WAL wraps RocksDB for writes but reads come straight from RocksDB,
    /// so streaming uses the same channel-driven pattern as
    /// [`RocksdbKVStore::scan_stream`].
    async fn scan_stream(&self, prefix: &[u8]) -> Result<BoxStream<'static, ScanStreamItem>> {
        let db = Arc::clone(&self.db);
        let prefix = prefix.to_vec();
        let (tx, rx) = tokio::sync::mpsc::channel::<ScanStreamItem>(64);
        tokio::task::spawn_blocking(move || {
            let iter = db.iterator(IteratorMode::From(&prefix, Direction::Forward));
            for item in iter {
                let send_result = match item {
                    Ok((k, v)) => {
                        if !k.starts_with(&prefix) {
                            break;
                        }
                        tx.blocking_send(Ok((k.to_vec(), v.to_vec())))
                    }
                    Err(e) => tx.blocking_send(Err(e.into())),
                };
                if send_result.is_err() {
                    break;
                }
            }
        });
        Ok(ReceiverStream::new(rx).boxed())
    }
}

// NOTE: gated on `rocksdb` because the test module mixes MemoryKVStore tests
// with KVStoreOptions/WalKVStore/RocksdbKVStore tests. Splitting the pure-memory
// tests into their own always-on module is tracked in docs/PLAN.md (Phase 2 debt).
#[cfg(all(test, feature = "rocksdb"))]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // ----- KVStoreOptions -------------------------------------------------

    #[test]
    fn test_kvstore_options_default() {
        let opts = KVStoreOptions::default();
        assert!(opts.create_if_missing);
        assert!(!opts.use_fsync);
        assert!(opts.wal_enabled);
        assert!(opts.wal_dir.is_none());
        assert_eq!(opts.bloom_filter_bits, 10);
        assert_eq!(opts.block_cache_size, 256 * 1024 * 1024);
        assert_eq!(opts.max_open_files, 5000);
        assert_eq!(opts.wal_max_file_size, 64 * 1024 * 1024);
    }

    // ----- MemoryKVStore: golden path & edges ----------------------------

    #[tokio::test]
    async fn test_memory_get_missing_returns_none() {
        let store = MemoryKVStore::new();
        assert!(store.get(b"missing").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_memory_put_overwrites_existing() {
        let store = MemoryKVStore::new();
        store.put(b"k", b"v1").await.unwrap();
        store.put(b"k", b"v2").await.unwrap();
        assert_eq!(store.get(b"k").await.unwrap(), Some(b"v2".to_vec()));
    }

    #[tokio::test]
    async fn test_memory_delete_missing_is_noop() {
        // Deleting a key that does not exist must succeed silently
        let store = MemoryKVStore::new();
        store.delete(b"never_inserted").await.unwrap();
        assert!(store.get(b"never_inserted").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_memory_put_then_delete_then_get() {
        let store = MemoryKVStore::new();
        store.put(b"k", b"v").await.unwrap();
        store.delete(b"k").await.unwrap();
        assert!(store.get(b"k").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_memory_put_empty_value_is_distinct_from_missing() {
        // An explicit empty value must round-trip and not be confused with absence
        let store = MemoryKVStore::new();
        store.put(b"k", b"").await.unwrap();
        assert_eq!(store.get(b"k").await.unwrap(), Some(Vec::new()));
    }

    #[tokio::test]
    async fn test_memory_batch_put_and_batch_get_order() {
        let store = MemoryKVStore::new();
        store
            .batch_put(vec![
                (b"a".to_vec(), b"1".to_vec()),
                (b"b".to_vec(), b"2".to_vec()),
                (b"c".to_vec(), b"3".to_vec()),
            ])
            .await
            .unwrap();

        // batch_get must preserve input order, including misses.
        let got = store
            .batch_get(&[
                b"c".to_vec(),
                b"missing".to_vec(),
                b"a".to_vec(),
                b"b".to_vec(),
            ])
            .await
            .unwrap();
        assert_eq!(
            got,
            vec![
                Some(b"3".to_vec()),
                None,
                Some(b"1".to_vec()),
                Some(b"2".to_vec()),
            ]
        );
    }

    #[tokio::test]
    async fn test_memory_batch_put_empty_is_noop() {
        let store = MemoryKVStore::new();
        store.batch_put(vec![]).await.unwrap();
        assert!(store.scan_prefix(b"").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_memory_batch_get_empty_input() {
        let store = MemoryKVStore::new();
        let got = store.batch_get(&[]).await.unwrap();
        assert!(got.is_empty());
    }

    #[tokio::test]
    async fn test_memory_scan_prefix_respects_boundary() {
        // Keys "user:" prefix must not return "user_extra:" entries.
        let store = MemoryKVStore::new();
        store.put(b"user:1", b"a").await.unwrap();
        store.put(b"user:2", b"b").await.unwrap();
        store.put(b"user_extra:9", b"x").await.unwrap();
        store.put(b"vendor:1", b"y").await.unwrap();

        let results = store.scan_prefix(b"user:").await.unwrap();
        assert_eq!(results.len(), 2);
        for (k, _) in &results {
            assert!(k.starts_with(b"user:"));
        }
    }

    #[tokio::test]
    async fn test_memory_scan_prefix_no_match_returns_empty() {
        let store = MemoryKVStore::new();
        store.put(b"a", b"1").await.unwrap();
        let results = store.scan_prefix(b"zzz").await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_memory_scan_prefix_empty_returns_all() {
        // Empty prefix is a degenerate but valid case (everything starts with "").
        let store = MemoryKVStore::new();
        store.put(b"a", b"1").await.unwrap();
        store.put(b"b", b"2").await.unwrap();
        let results = store.scan_prefix(b"").await.unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_memory_scan_prefix_returns_sorted_keys() {
        // BTreeMap-backed store should return keys in lexicographic order.
        let store = MemoryKVStore::new();
        for k in [b"c", b"a", b"b"] {
            store.put(k, b"v").await.unwrap();
        }
        let results = store.scan_prefix(b"").await.unwrap();
        let keys: Vec<&[u8]> = results.iter().map(|(k, _)| k.as_slice()).collect();
        assert_eq!(keys, vec![&b"a"[..], &b"b"[..], &b"c"[..]]);
    }

    #[tokio::test]
    async fn test_memory_scan_prefix_limited_caps_results() {
        let store = MemoryKVStore::new();
        for i in 0..10u8 {
            store.put(&[b'p', i], &[i]).await.unwrap();
        }

        let results = store.scan_prefix_limited(b"p", Some(4)).await.unwrap();
        assert_eq!(results.len(), 4);
    }

    #[tokio::test]
    async fn test_memory_scan_with_filter_visit_stops_when_visitor_returns_false() {
        let store = MemoryKVStore::new();
        for i in 0..10u8 {
            store.put(&[b'p', i], &[i]).await.unwrap();
        }

        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let visitor_seen = seen.clone();
        let visited = store
            .scan_with_filter_visit(
                b"p",
                Box::new(|_, _| true),
                Box::new(move |_, value| {
                    let mut seen = visitor_seen.lock().unwrap();
                    seen.push(value[0]);
                    seen.len() < 2
                }),
                None,
            )
            .await
            .unwrap();

        assert_eq!(visited, 2);
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_memory_scan_with_filter_limit_zero_returns_empty() {
        let store = MemoryKVStore::new();
        store.put(b"k1", b"v1").await.unwrap();
        store.put(b"k2", b"v2").await.unwrap();

        let filter: FilterFn = Box::new(|_, _| true);
        let results = store.scan_with_filter(b"k", filter, Some(0)).await.unwrap();
        assert!(results.is_empty(), "limit=0 must short-circuit");
    }

    #[tokio::test]
    async fn test_memory_scan_with_filter_limit_caps_results() {
        let store = MemoryKVStore::new();
        for i in 0..10u8 {
            store.put(&[b'k', i], &[i]).await.unwrap();
        }

        let filter: FilterFn = Box::new(|_, _| true);
        let results = store.scan_with_filter(b"k", filter, Some(3)).await.unwrap();
        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn test_memory_scan_with_filter_excludes_non_matching() {
        let store = MemoryKVStore::new();
        store.put(b"k1", b"odd").await.unwrap();
        store.put(b"k2", b"even").await.unwrap();
        store.put(b"k3", b"odd").await.unwrap();

        let filter: FilterFn = Box::new(|_, v| v == b"odd");
        let results = store.scan_with_filter(b"k", filter, None).await.unwrap();
        assert_eq!(results.len(), 2);
        for (_, v) in &results {
            assert_eq!(v, b"odd");
        }
    }

    #[tokio::test]
    async fn test_memory_scan_with_filter_none_limit_means_unbounded() {
        let store = MemoryKVStore::new();
        for i in 0..5u8 {
            store.put(&[b'p', i], &[i]).await.unwrap();
        }
        let filter: FilterFn = Box::new(|_, _| true);
        let results = store.scan_with_filter(b"p", filter, None).await.unwrap();
        assert_eq!(results.len(), 5);
    }

    #[tokio::test]
    async fn test_memory_scan_stream_yields_all_matching_keys() {
        let store = MemoryKVStore::new();
        for i in 0..5u8 {
            store.put(&[b'p', i], &[i]).await.unwrap();
        }
        store.put(b"other", b"x").await.unwrap();

        let mut stream = store.scan_stream(b"p").await.unwrap();
        let mut collected = Vec::new();
        while let Some(item) = stream.next().await {
            collected.push(item.unwrap());
        }
        assert_eq!(collected.len(), 5);
        assert!(collected.iter().all(|(k, _)| k.starts_with(b"p")));
    }

    #[tokio::test]
    async fn test_memory_scan_stream_early_drop_does_not_panic() {
        // Dropping the stream after consuming only a prefix must work.
        let store = MemoryKVStore::new();
        for i in 0..1000u32 {
            let key = format!("p{:04}", i);
            store.put(key.as_bytes(), &[]).await.unwrap();
        }
        let mut stream = store.scan_stream(b"p").await.unwrap();
        let _first = stream.next().await.unwrap().unwrap();
        drop(stream);
    }

    #[tokio::test]
    async fn test_memory_default_is_empty() {
        let store: MemoryKVStore = MemoryKVStore::default();
        assert!(store.scan_prefix(b"").await.unwrap().is_empty());
    }

    // ----- WalKVStore: durability & recovery -----------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_wal_kvstore_recovers_after_reopen() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("db");

        // Phase 1: write data, then drop store cleanly
        {
            let store = WalKVStore::open(&db_path, KVStoreOptions::default()).unwrap();
            store.put(b"user:1", b"alice").await.unwrap();
            store.put(b"user:2", b"bob").await.unwrap();
            store.delete(b"user:1").await.unwrap();
            store.sync().unwrap();
            assert!(store.current_lsn() >= 3);
        }

        // Phase 2: reopen and verify state survives
        let store2 = WalKVStore::open(&db_path, KVStoreOptions::default()).unwrap();
        assert!(store2.get(b"user:1").await.unwrap().is_none());
        assert_eq!(store2.get(b"user:2").await.unwrap(), Some(b"bob".to_vec()));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_wal_kvstore_batch_put_persists() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("db");

        let store = WalKVStore::open(&db_path, KVStoreOptions::default()).unwrap();
        store
            .batch_put(vec![
                (b"a".to_vec(), b"1".to_vec()),
                (b"b".to_vec(), b"2".to_vec()),
                (b"c".to_vec(), b"3".to_vec()),
            ])
            .await
            .unwrap();

        let got = store
            .batch_get(&[b"a".to_vec(), b"b".to_vec(), b"c".to_vec()])
            .await
            .unwrap();
        assert_eq!(
            got,
            vec![
                Some(b"1".to_vec()),
                Some(b"2".to_vec()),
                Some(b"3".to_vec()),
            ]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_wal_kvstore_scan_with_filter_limit_early_exit() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("db");
        let store = WalKVStore::open(&db_path, KVStoreOptions::default()).unwrap();

        for i in 0..20u8 {
            store.put(&[b'p', i], &[i]).await.unwrap();
        }

        let filter: FilterFn = Box::new(|_, _| true);
        let results = store.scan_with_filter(b"p", filter, Some(5)).await.unwrap();
        assert_eq!(results.len(), 5);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_wal_kvstore_scan_with_filter_limit_zero_returns_empty() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("db");
        let store = WalKVStore::open(&db_path, KVStoreOptions::default()).unwrap();

        store.put(b"p1", b"1").await.unwrap();

        let filter: FilterFn = Box::new(|_, _| true);
        let results = store.scan_with_filter(b"p", filter, Some(0)).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_wal_kvstore_scan_with_filter_visit_stops_early() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("db");
        let store = WalKVStore::open(&db_path, KVStoreOptions::default()).unwrap();

        for i in 0..10u8 {
            store.put(&[b'p', i], &[i]).await.unwrap();
        }

        let mut seen = 0usize;
        let visited = store
            .scan_with_filter_visit(
                b"p",
                Box::new(|_, _| true),
                Box::new(move |_, _| {
                    seen += 1;
                    seen < 2
                }),
                None,
            )
            .await
            .unwrap();

        assert_eq!(visited, 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_wal_kvstore_cleanup_wal_does_not_corrupt_db() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("db");
        let store = WalKVStore::open(&db_path, KVStoreOptions::default()).unwrap();

        store.put(b"k1", b"v1").await.unwrap();
        store.put(b"k2", b"v2").await.unwrap();
        let lsn = store.current_lsn();

        // Cleanup all WAL up to current LSN — committed data must remain queryable.
        store.cleanup_wal(lsn + 1).unwrap();

        assert_eq!(store.get(b"k1").await.unwrap(), Some(b"v1".to_vec()));
        assert_eq!(store.get(b"k2").await.unwrap(), Some(b"v2".to_vec()));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_wal_kvstore_open_fails_on_missing_when_create_disabled() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("nope");
        let opts = KVStoreOptions {
            create_if_missing: false,
            ..KVStoreOptions::default()
        };
        let result = WalKVStore::open(&db_path, opts);
        assert!(
            result.is_err(),
            "opening missing DB with create_if_missing=false must fail"
        );
    }

    // ----- RocksdbKVStore -------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_rocksdb_kvstore_basic_round_trip() {
        let dir = tempdir().unwrap();
        let store = RocksdbKVStore::open(dir.path(), KVStoreOptions::default()).unwrap();

        store.put(b"k", b"v").await.unwrap();
        assert_eq!(store.get(b"k").await.unwrap(), Some(b"v".to_vec()));

        store.delete(b"k").await.unwrap();
        assert!(store.get(b"k").await.unwrap().is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_rocksdb_kvstore_scan_with_filter_predicate_pushdown() {
        let dir = tempdir().unwrap();
        let store = RocksdbKVStore::open(dir.path(), KVStoreOptions::default()).unwrap();

        for i in 0..10u8 {
            store.put(&[b'k', i], &[i]).await.unwrap();
        }

        let filter: FilterFn = Box::new(|_, v| !v.is_empty() && v[0] % 2 == 0);
        let results = store.scan_with_filter(b"k", filter, None).await.unwrap();
        // Even values: 0,2,4,6,8 → 5 results
        assert_eq!(results.len(), 5);
        for (_, v) in &results {
            assert!(v[0] % 2 == 0);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_rocksdb_kvstore_scan_stream_yields_all() {
        let dir = tempdir().unwrap();
        let store = RocksdbKVStore::open(dir.path(), KVStoreOptions::default()).unwrap();

        for i in 0..10u8 {
            store.put(&[b'k', i], &[i]).await.unwrap();
        }

        let mut stream = store.scan_stream(b"k").await.unwrap();
        let mut count = 0;
        while let Some(item) = stream.next().await {
            item.unwrap();
            count += 1;
        }
        assert_eq!(count, 10);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_rocksdb_kvstore_scan_stream_early_drop_releases_iterator() {
        let dir = tempdir().unwrap();
        let store = RocksdbKVStore::open(dir.path(), KVStoreOptions::default()).unwrap();

        for i in 0..2000u32 {
            let key = format!("k{:05}", i);
            store.put(key.as_bytes(), &[]).await.unwrap();
        }

        let mut stream = store.scan_stream(b"k").await.unwrap();
        let _ = stream.next().await.unwrap().unwrap();
        // Dropping the stream must terminate the backing task without panic.
        drop(stream);
        // Subsequent scan still works (DB not poisoned).
        let mut stream2 = store.scan_stream(b"k").await.unwrap();
        let _ = stream2.next().await.unwrap().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_rocksdb_kvstore_scan_prefix_limited_caps_results() {
        let dir = tempdir().unwrap();
        let store = RocksdbKVStore::open(dir.path(), KVStoreOptions::default()).unwrap();

        for i in 0..10u8 {
            store.put(&[b'p', i], &[i]).await.unwrap();
        }

        let results = store.scan_prefix_limited(b"p", Some(3)).await.unwrap();
        assert_eq!(results.len(), 3);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_rocksdb_kvstore_scan_with_filter_limit_zero_returns_empty() {
        let dir = tempdir().unwrap();
        let store = RocksdbKVStore::open(dir.path(), KVStoreOptions::default()).unwrap();

        store.put(b"p1", b"1").await.unwrap();

        let filter: FilterFn = Box::new(|_, _| true);
        let results = store.scan_with_filter(b"p", filter, Some(0)).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_rocksdb_kvstore_scan_with_filter_visit_stops_early() {
        let dir = tempdir().unwrap();
        let store = RocksdbKVStore::open(dir.path(), KVStoreOptions::default()).unwrap();

        for i in 0..10u8 {
            store.put(&[b'p', i], &[i]).await.unwrap();
        }

        let mut seen = 0usize;
        let visited = store
            .scan_with_filter_visit(
                b"p",
                Box::new(|_, _| true),
                Box::new(move |_, _| {
                    seen += 1;
                    seen < 2
                }),
                None,
            )
            .await
            .unwrap();

        assert_eq!(visited, 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_rocksdb_kvstore_disabled_bloom_filter_still_works() {
        // Bloom filter is an optimization, not a correctness gate
        let dir = tempdir().unwrap();
        let opts = KVStoreOptions {
            bloom_filter_bits: 0,
            block_cache_size: 0,
            ..KVStoreOptions::default()
        };
        let store = RocksdbKVStore::open(dir.path(), opts).unwrap();

        store.put(b"k", b"v").await.unwrap();
        assert_eq!(store.get(b"k").await.unwrap(), Some(b"v".to_vec()));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_rocksdb_kvstore_open_fails_when_create_if_missing_false() {
        let dir = tempdir().unwrap();
        let nonexistent = dir.path().join("nope");
        let opts = KVStoreOptions {
            create_if_missing: false,
            ..KVStoreOptions::default()
        };
        assert!(RocksdbKVStore::open(&nonexistent, opts).is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_rocksdb_kvstore_batch_get_preserves_order_with_holes() {
        let dir = tempdir().unwrap();
        let store = RocksdbKVStore::open(dir.path(), KVStoreOptions::default()).unwrap();

        store.put(b"present1", b"a").await.unwrap();
        store.put(b"present2", b"b").await.unwrap();

        let got = store
            .batch_get(&[
                b"missing".to_vec(),
                b"present2".to_vec(),
                b"present1".to_vec(),
            ])
            .await
            .unwrap();
        assert_eq!(got, vec![None, Some(b"b".to_vec()), Some(b"a".to_vec())]);
    }
}

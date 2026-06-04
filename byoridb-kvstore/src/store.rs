// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use crate::error::Result;
use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
use redb::{Builder, Database, ReadableDatabase, TableDefinition};
use std::path::Path;
use std::sync::Arc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::info;

/// Single KV table. The store presents one flat byte keyspace, so all rows
/// live in one redb table; prefix scans are range queries over it.
const KV_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("kv");

/// Item yielded by [`KVStore::scan_stream`]. Each `(key, value)` is owned so
/// the stream can outlive the underlying iterator.
pub type ScanStreamItem = Result<(Vec<u8>, Vec<u8>)>;

/// Options for opening a [`RedbKVStore`].
///
/// redb is a pure-Rust embedded store with built-in ACID durability, so the
/// surface is far smaller than the old RocksDB options. Only `create_if_missing`
/// and `cache_size` have effect today; `use_fsync` is retained for callers and
/// reserved for a future `Durability::Eventual` toggle (commits currently always
/// use `Durability::Immediate`, which fsyncs on every commit).
#[derive(Debug, Clone)]
pub struct KVStoreOptions {
    pub create_if_missing: bool,
    /// redb page cache size in bytes (256MB default).
    pub cache_size: usize,
    /// Reserved: when false, commits could map to `Durability::Eventual`.
    /// Today every commit uses `Durability::Immediate` regardless.
    pub use_fsync: bool,
}

impl Default for KVStoreOptions {
    fn default() -> Self {
        KVStoreOptions {
            create_if_missing: true,
            cache_size: 256 * 1024 * 1024, // 256MB page cache
            use_fsync: false,
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

/// redb-backed [`KVStore`]. Pure Rust, single-file, ACID.
///
/// redb is synchronous and serializes write transactions internally, so every
/// operation runs inside `spawn_blocking` (no extra writer mutex needed) and
/// reads use MVCC read transactions that never block writers.
pub struct RedbKVStore {
    db: Arc<Database>,
}

impl RedbKVStore {
    /// Open (or create) a store. `path` is treated as a **directory**; the redb
    /// file lives at `<path>/data.redb`. This preserves the directory-based
    /// data-path contract the storage/meta layers and backup tooling assume.
    pub fn open<P: AsRef<Path>>(path: P, opts: KVStoreOptions) -> Result<Self> {
        let dir = path.as_ref();
        let file = dir.join("data.redb");

        let db = if opts.create_if_missing {
            std::fs::create_dir_all(dir)?;
            Builder::new()
                .set_cache_size(opts.cache_size)
                .create(&file)?
        } else {
            Database::open(&file)?
        };

        // A fresh database has no tables, so begin_read().open_table() would
        // fail. Materialize the table once up front.
        let wtx = db.begin_write()?;
        {
            wtx.open_table(KV_TABLE)?;
        }
        wtx.commit()?;

        info!(
            "redb opened at {:?} (cache_size={}MB)",
            file,
            opts.cache_size / (1024 * 1024)
        );
        Ok(RedbKVStore { db: Arc::new(db) })
    }

    /// Construct from an already-open database (used by tooling/tests).
    pub fn with_db(db: Arc<Database>) -> Self {
        RedbKVStore { db }
    }
}

#[async_trait]
impl KVStore for RedbKVStore {
    async fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let db = Arc::clone(&self.db);
        let key = key.to_vec();
        tokio::task::spawn_blocking(move || {
            let rtx = db.begin_read()?;
            let table = rtx.open_table(KV_TABLE)?;
            Ok(table.get(key.as_slice())?.map(|g| g.value().to_vec()))
        })
        .await?
    }

    async fn put(&self, key: &[u8], value: &[u8]) -> Result<()> {
        let db = Arc::clone(&self.db);
        let key = key.to_vec();
        let value = value.to_vec();
        tokio::task::spawn_blocking(move || {
            let wtx = db.begin_write()?;
            {
                let mut table = wtx.open_table(KV_TABLE)?;
                table.insert(key.as_slice(), value.as_slice())?;
            }
            wtx.commit()?;
            Ok(())
        })
        .await?
    }

    async fn delete(&self, key: &[u8]) -> Result<()> {
        let db = Arc::clone(&self.db);
        let key = key.to_vec();
        tokio::task::spawn_blocking(move || {
            let wtx = db.begin_write()?;
            {
                let mut table = wtx.open_table(KV_TABLE)?;
                table.remove(key.as_slice())?;
            }
            wtx.commit()?;
            Ok(())
        })
        .await?
    }

    /// All pairs are written in a single transaction → atomic all-or-nothing.
    async fn batch_put(&self, pairs: Vec<(Vec<u8>, Vec<u8>)>) -> Result<()> {
        let db = Arc::clone(&self.db);
        tokio::task::spawn_blocking(move || {
            let wtx = db.begin_write()?;
            {
                let mut table = wtx.open_table(KV_TABLE)?;
                for (key, value) in &pairs {
                    table.insert(key.as_slice(), value.as_slice())?;
                }
            }
            wtx.commit()?;
            Ok(())
        })
        .await?
    }

    /// Reads keys in input order under a single snapshot. Misses stay `None`.
    async fn batch_get(&self, keys: &[Vec<u8>]) -> Result<Vec<Option<Vec<u8>>>> {
        let db = Arc::clone(&self.db);
        let keys = keys.to_vec();
        tokio::task::spawn_blocking(move || {
            let rtx = db.begin_read()?;
            let table = rtx.open_table(KV_TABLE)?;
            let mut output = Vec::with_capacity(keys.len());
            for key in &keys {
                output.push(table.get(key.as_slice())?.map(|g| g.value().to_vec()));
            }
            Ok(output)
        })
        .await?
    }

    async fn scan_prefix(&self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let db = Arc::clone(&self.db);
        let prefix = prefix.to_vec();
        tokio::task::spawn_blocking(move || {
            let rtx = db.begin_read()?;
            let table = rtx.open_table(KV_TABLE)?;
            let mut results = Vec::new();
            for entry in table.range(prefix.as_slice()..)? {
                let (k, v) = entry?;
                let kb = k.value();
                if !kb.starts_with(&prefix) {
                    break;
                }
                results.push((kb.to_vec(), v.value().to_vec()));
            }
            Ok(results)
        })
        .await?
    }

    /// Predicate pushdown: filter is applied during range iteration, with an
    /// early exit once `limit` matches are collected.
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
            let rtx = db.begin_read()?;
            let table = rtx.open_table(KV_TABLE)?;
            let mut results = Vec::new();
            for entry in table.range(prefix.as_slice()..)? {
                let (k, v) = entry?;
                let kb = k.value();
                if !kb.starts_with(&prefix) {
                    break;
                }
                let vb = v.value();
                if filter(kb, vb) {
                    results.push((kb.to_vec(), vb.to_vec()));
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
            let rtx = db.begin_read()?;
            let table = rtx.open_table(KV_TABLE)?;
            let mut visited = 0;
            for entry in table.range(prefix.as_slice()..)? {
                let (k, v) = entry?;
                let kb = k.value();
                if !kb.starts_with(&prefix) {
                    break;
                }
                if filter(kb, v.value()) {
                    visited += 1;
                    if !visitor(kb, v.value()) || visited >= limit {
                        break;
                    }
                }
            }
            Ok(visited)
        })
        .await?
    }

    /// Channel-driven streaming scan. A `spawn_blocking` task owns the read
    /// transaction, table, and range iterator and forwards rows over a bounded
    /// mpsc channel. When the consumer drops the stream the send errors, the
    /// loop breaks, and the transaction is dropped — partial consumption never
    /// materializes the whole range.
    async fn scan_stream(&self, prefix: &[u8]) -> Result<BoxStream<'static, ScanStreamItem>> {
        let db = Arc::clone(&self.db);
        let prefix = prefix.to_vec();
        let (tx, rx) = tokio::sync::mpsc::channel::<ScanStreamItem>(64);
        tokio::task::spawn_blocking(move || {
            let outcome = (|| -> Result<()> {
                let rtx = db.begin_read()?;
                let table = rtx.open_table(KV_TABLE)?;
                for entry in table.range(prefix.as_slice()..)? {
                    let (k, v) = entry?;
                    let kb = k.value();
                    if !kb.starts_with(&prefix) {
                        break;
                    }
                    if tx
                        .blocking_send(Ok((kb.to_vec(), v.value().to_vec())))
                        .is_err()
                    {
                        // consumer dropped the stream → stop early
                        break;
                    }
                }
                Ok(())
            })();
            if let Err(e) = outcome {
                let _ = tx.blocking_send(Err(e));
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // ----- KVStoreOptions -------------------------------------------------

    #[test]
    fn test_kvstore_options_default() {
        let opts = KVStoreOptions::default();
        assert!(opts.create_if_missing);
        assert!(!opts.use_fsync);
        assert_eq!(opts.cache_size, 256 * 1024 * 1024);
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

    // ----- RedbKVStore: durability & correctness -------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_redb_kvstore_basic_round_trip() {
        let dir = tempdir().unwrap();
        let store = RedbKVStore::open(dir.path(), KVStoreOptions::default()).unwrap();

        store.put(b"k", b"v").await.unwrap();
        assert_eq!(store.get(b"k").await.unwrap(), Some(b"v".to_vec()));

        store.delete(b"k").await.unwrap();
        assert!(store.get(b"k").await.unwrap().is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_redb_kvstore_recovers_after_reopen() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("db");

        {
            let store = RedbKVStore::open(&db_path, KVStoreOptions::default()).unwrap();
            store.put(b"user:1", b"alice").await.unwrap();
            store.put(b"user:2", b"bob").await.unwrap();
            store.delete(b"user:1").await.unwrap();
        }

        // Reopen and verify committed state survives.
        let store2 = RedbKVStore::open(&db_path, KVStoreOptions::default()).unwrap();
        assert!(store2.get(b"user:1").await.unwrap().is_none());
        assert_eq!(store2.get(b"user:2").await.unwrap(), Some(b"bob".to_vec()));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_redb_kvstore_put_empty_value_distinct_from_missing() {
        let dir = tempdir().unwrap();
        let store = RedbKVStore::open(dir.path(), KVStoreOptions::default()).unwrap();
        store.put(b"k", b"").await.unwrap();
        assert_eq!(store.get(b"k").await.unwrap(), Some(Vec::new()));
        assert!(store.get(b"absent").await.unwrap().is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_redb_kvstore_batch_put_atomic_and_get_order() {
        let dir = tempdir().unwrap();
        let store = RedbKVStore::open(dir.path(), KVStoreOptions::default()).unwrap();
        store
            .batch_put(vec![
                (b"a".to_vec(), b"1".to_vec()),
                (b"b".to_vec(), b"2".to_vec()),
                (b"c".to_vec(), b"3".to_vec()),
            ])
            .await
            .unwrap();

        let got = store
            .batch_get(&[b"c".to_vec(), b"missing".to_vec(), b"a".to_vec()])
            .await
            .unwrap();
        assert_eq!(got, vec![Some(b"3".to_vec()), None, Some(b"1".to_vec())]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_redb_kvstore_scan_prefix_respects_boundary() {
        let dir = tempdir().unwrap();
        let store = RedbKVStore::open(dir.path(), KVStoreOptions::default()).unwrap();
        store.put(b"user:1", b"a").await.unwrap();
        store.put(b"user_extra:9", b"x").await.unwrap();
        store.put(b"vendor:1", b"y").await.unwrap();

        let results = store.scan_prefix(b"user:").await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].0.starts_with(b"user:"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_redb_kvstore_scan_with_filter_predicate_pushdown() {
        let dir = tempdir().unwrap();
        let store = RedbKVStore::open(dir.path(), KVStoreOptions::default()).unwrap();

        for i in 0..10u8 {
            store.put(&[b'k', i], &[i]).await.unwrap();
        }

        let filter: FilterFn = Box::new(|_, v| !v.is_empty() && v[0] % 2 == 0);
        let results = store.scan_with_filter(b"k", filter, None).await.unwrap();
        assert_eq!(results.len(), 5);
        for (_, v) in &results {
            assert!(v[0] % 2 == 0);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_redb_kvstore_scan_with_filter_limit_zero_returns_empty() {
        let dir = tempdir().unwrap();
        let store = RedbKVStore::open(dir.path(), KVStoreOptions::default()).unwrap();
        store.put(b"p1", b"1").await.unwrap();

        let filter: FilterFn = Box::new(|_, _| true);
        let results = store.scan_with_filter(b"p", filter, Some(0)).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_redb_kvstore_scan_prefix_limited_caps_results() {
        let dir = tempdir().unwrap();
        let store = RedbKVStore::open(dir.path(), KVStoreOptions::default()).unwrap();

        for i in 0..10u8 {
            store.put(&[b'p', i], &[i]).await.unwrap();
        }

        let results = store.scan_prefix_limited(b"p", Some(3)).await.unwrap();
        assert_eq!(results.len(), 3);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_redb_kvstore_scan_with_filter_visit_stops_early() {
        let dir = tempdir().unwrap();
        let store = RedbKVStore::open(dir.path(), KVStoreOptions::default()).unwrap();

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
    async fn test_redb_kvstore_scan_stream_yields_all() {
        let dir = tempdir().unwrap();
        let store = RedbKVStore::open(dir.path(), KVStoreOptions::default()).unwrap();

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
    async fn test_redb_kvstore_scan_stream_early_drop_releases_iterator() {
        let dir = tempdir().unwrap();
        let store = RedbKVStore::open(dir.path(), KVStoreOptions::default()).unwrap();

        for i in 0..2000u32 {
            let key = format!("k{:05}", i);
            store.put(key.as_bytes(), &[]).await.unwrap();
        }

        let mut stream = store.scan_stream(b"k").await.unwrap();
        let _ = stream.next().await.unwrap().unwrap();
        drop(stream);
        // Subsequent scan still works (DB not poisoned).
        let mut stream2 = store.scan_stream(b"k").await.unwrap();
        let _ = stream2.next().await.unwrap().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_redb_kvstore_open_fails_when_create_if_missing_false() {
        let dir = tempdir().unwrap();
        let nonexistent = dir.path().join("nope");
        let opts = KVStoreOptions {
            create_if_missing: false,
            ..KVStoreOptions::default()
        };
        assert!(RedbKVStore::open(&nonexistent, opts).is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_redb_kvstore_zero_cache_still_works() {
        // cache_size is an optimization, not a correctness gate.
        let dir = tempdir().unwrap();
        let opts = KVStoreOptions {
            cache_size: 0,
            ..KVStoreOptions::default()
        };
        let store = RedbKVStore::open(dir.path(), opts).unwrap();
        store.put(b"k", b"v").await.unwrap();
        assert_eq!(store.get(b"k").await.unwrap(), Some(b"v".to_vec()));
    }
}

// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use crate::error::Result;
use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
use redb::{Builder, Database, Durability, ReadableDatabase, ReadableTable, TableDefinition};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::info;

/// Single KV table. The store presents one flat byte keyspace, so all rows
/// live in one redb table; prefix scans are range queries over it.
const KV_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("kv");
/// Separate table for bitemporal version history (T-트랙). Physically isolated
/// from `KV_TABLE` so current-view full scans don't share B-tree pages / page
/// cache with bulk history — the prototype (`examples/temporal_readbench.rs`)
/// showed co-residence in one table regressed current-view prefix-scan ~2x.
const HISTORY_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("history");

/// Item yielded by [`KVStore::scan_stream`]. Each `(key, value)` is owned so
/// the stream can outlive the underlying iterator.
pub type ScanStreamItem = Result<(Vec<u8>, Vec<u8>)>;

/// Options for opening a [`RedbKVStore`].
///
/// redb is a pure-Rust embedded store with built-in ACID durability, so the
/// surface is far smaller than the old RocksDB options.
#[derive(Debug, Clone)]
pub struct KVStoreOptions {
    pub create_if_missing: bool,
    /// redb page cache size in bytes (256MB default).
    pub cache_size: usize,
    /// When true (default), every write commit uses `Durability::Immediate`
    /// (fsync per commit). When false, writes use relaxed durability
    /// (`Durability::None`, no per-commit fsync) with a periodic `Immediate`
    /// checkpoint — much faster for bulk loads, but a crash loses recent
    /// commits. Only set false for re-loadable bulk imports.
    pub use_fsync: bool,
}

impl Default for KVStoreOptions {
    fn default() -> Self {
        KVStoreOptions {
            create_if_missing: true,
            cache_size: 256 * 1024 * 1024, // 256MB page cache
            use_fsync: true,               // Immediate durability (fsync per commit) by default
        }
    }
}

/// Filter function type for scan_with_filter
/// Takes (key, value) and returns true if the row should be included
pub type FilterFn = Box<dyn Fn(&[u8], &[u8]) -> bool + Send + Sync>;

/// Visitor function type for streaming scans.
/// Return true to continue scanning, false to stop early.
pub type ScanVisitorFn = Box<dyn FnMut(&[u8], &[u8]) -> bool + Send>;

/// Sentinel for an open (still-valid / "Now"/∞) interval end. See T-트랙 design.
pub const VALID_OPEN: i64 = i64::MAX;

/// One stored bitemporal version of an entity (T-트랙, asserted-facts-only).
/// `valid_from`/`valid_to` = real-world validity as a half-open `[from, to)`
/// interval (`to == VALID_OPEN` means still valid). `tx` = transaction time
/// (when the system recorded it). `value` = the entity payload at that version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionRecord {
    pub valid_from: i64,
    pub valid_to: i64,
    pub tx: i64,
    pub value: Vec<u8>,
}

// Order-preserving DESCENDING 8-byte encoding of an i64, so that within an
// entity's history prefix, larger valid_from / tx sort first (newest-first).
fn desc_i64(v: i64) -> [u8; 8] {
    (!((v as u64) ^ (1u64 << 63))).to_be_bytes()
}

fn undesc_i64(b: &[u8]) -> i64 {
    let mut arr = [0u8; 8];
    arr.copy_from_slice(b);
    ((!u64::from_be_bytes(arr)) ^ (1u64 << 63)) as i64
}

/// History key = `entity_key || 0x00 || desc(valid_from) || desc(tx)`.
/// Entity keys are ASCII (`sp:vertex:…`) so the 0x00 separator can't collide.
fn history_key(entity_key: &[u8], valid_from: i64, tx: i64) -> Vec<u8> {
    let mut k = Vec::with_capacity(entity_key.len() + 1 + 16);
    k.extend_from_slice(entity_key);
    k.push(0x00);
    k.extend_from_slice(&desc_i64(valid_from));
    k.extend_from_slice(&desc_i64(tx));
    k
}

fn history_prefix(entity_key: &[u8]) -> Vec<u8> {
    let mut k = Vec::with_capacity(entity_key.len() + 1);
    k.extend_from_slice(entity_key);
    k.push(0x00);
    k
}

/// Recover `(valid_from, tx)` from the trailing 16 bytes of a [`history_key`].
fn parse_history_key_times(key: &[u8]) -> Option<(i64, i64)> {
    if key.len() < 16 {
        return None;
    }
    let vf = undesc_i64(&key[key.len() - 16..key.len() - 8]);
    let tx = undesc_i64(&key[key.len() - 8..]);
    Some((vf, tx))
}

/// History value = `valid_to (8B BE) || payload`.
fn encode_history_value(valid_to: i64, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(8 + payload.len());
    v.extend_from_slice(&valid_to.to_be_bytes());
    v.extend_from_slice(payload);
    v
}

fn decode_history_value(v: &[u8]) -> Option<(i64, Vec<u8>)> {
    if v.len() < 8 {
        return None;
    }
    let mut arr = [0u8; 8];
    arr.copy_from_slice(&v[..8]);
    Some((i64::from_be_bytes(arr), v[8..].to_vec()))
}

#[async_trait]
pub trait KVStore: Send + Sync {
    async fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>>;
    async fn put(&self, key: &[u8], value: &[u8]) -> Result<()>;
    /// Atomically insert `value` only when `key` is absent.
    ///
    /// Returns `None` when this call inserted the value, or the existing value
    /// when another writer already owns the key. Implementations must perform
    /// the existence check and insertion under one write transaction/lock.
    async fn put_if_absent(&self, key: &[u8], value: &[u8]) -> Result<Option<Vec<u8>>>;
    async fn delete(&self, key: &[u8]) -> Result<()>;
    async fn batch_put(&self, pairs: Vec<(Vec<u8>, Vec<u8>)>) -> Result<()>;
    /// Delete many keys in a single transaction (missing keys are ignored).
    async fn batch_delete(&self, keys: Vec<Vec<u8>>) -> Result<()>;

    // ---- Bitemporal version history (T-트랙, asserted-facts-only) ----

    /// Append one immutable version of `entity_key` to the history keyspace.
    /// Never mutates existing rows (append-only). `valid_to == VALID_OPEN` marks
    /// a still-open interval. Physically separate from the current-view keyspace,
    /// so it never affects current-view reads.
    async fn put_version(
        &self,
        entity_key: &[u8],
        valid_from: i64,
        valid_to: i64,
        tx: i64,
        value: &[u8],
    ) -> Result<()>;

    /// All stored versions of `entity_key`, newest-first (valid_from desc, tx desc).
    async fn scan_history(&self, entity_key: &[u8]) -> Result<Vec<VersionRecord>>;

    /// Distinct entity keys with at least one history version under
    /// `entity_prefix` (a current-view key prefix), ascending. Includes entities
    /// whose current-view key no longer exists (deleted → tombstoned), which is
    /// what point-in-time reads over a key *range* need — e.g. edge `AS OF`
    /// where ranking/edge-type is enumerated rather than known.
    async fn scan_history_entity_keys(&self, entity_prefix: &[u8]) -> Result<Vec<Vec<u8>>>;

    /// Append many versions in one transaction (bulk write path). Each tuple is
    /// `(entity_key, valid_from, valid_to, tx, value)`. Append-only.
    async fn batch_put_version(
        &self,
        versions: Vec<(Vec<u8>, i64, i64, i64, Vec<u8>)>,
    ) -> Result<()>;

    /// Atomically apply current-view puts/deletes AND history version appends in
    /// ONE transaction (T-트랙 v1.1: dual-write 원자성). A crash can no longer
    /// leave the current view and its history disagreeing. `versions` tuples are
    /// `(entity_key, valid_from, valid_to, tx, value)`, append-only.
    ///
    /// The default is a sequential best-effort fallback for backends without
    /// cross-keyspace transactions; Redb/Memory override with a truly atomic apply.
    async fn batch_apply(
        &self,
        puts: Vec<(Vec<u8>, Vec<u8>)>,
        deletes: Vec<Vec<u8>>,
        versions: Vec<(Vec<u8>, i64, i64, i64, Vec<u8>)>,
    ) -> Result<()> {
        if !puts.is_empty() {
            self.batch_put(puts).await?;
        }
        if !deletes.is_empty() {
            self.batch_delete(deletes).await?;
        }
        if !versions.is_empty() {
            self.batch_put_version(versions).await?;
        }
        Ok(())
    }

    /// Resolve the value of `entity_key` as-of real-world `valid_at`, according to
    /// knowledge recorded up to transaction time `tx_at`: among versions with
    /// `tx <= tx_at` whose `[valid_from, valid_to)` covers `valid_at`, pick the one
    /// with the greatest `(valid_from, tx)`. Backend-agnostic default over
    /// [`KVStore::scan_history`] — O(versions); ordered backends override with a
    /// seek (keys sort newest-first, so the range starting at `(valid_at, tx_at)`
    /// visits candidates in exactly the preference order).
    async fn get_as_of(
        &self,
        entity_key: &[u8],
        valid_at: i64,
        tx_at: i64,
    ) -> Result<Option<Vec<u8>>> {
        let versions = self.scan_history(entity_key).await?;
        let mut best: Option<&VersionRecord> = None;
        for v in &versions {
            if v.tx <= tx_at && v.valid_from <= valid_at && valid_at < v.valid_to {
                match best {
                    Some(b) if (b.valid_from, b.tx) >= (v.valid_from, v.tx) => {}
                    _ => best = Some(v),
                }
            }
        }
        Ok(best.map(|v| v.value.clone()))
    }

    /// Force a durable (fsync, 2-phase) checkpoint so the next `open()` finds a
    /// clean shutdown and skips the expensive full-repair scan.
    ///
    /// redb's `open()` runs a full repair (3 scans of the file) whenever the
    /// last transaction wasn't a 2-phase commit that updated the allocator-state
    /// table. On a large dataset that repair takes many minutes, so a server
    /// MUST call this on graceful shutdown (and a bulk loader at the end of a
    /// relaxed-durability load) to leave the store clean. Default is a no-op for
    /// backends that are always durable (e.g. in-memory).
    async fn checkpoint(&self) -> Result<()> {
        Ok(())
    }

    /// Delete every key under `prefix`, in chunks of up to `chunk` keys per
    /// commit. Built for `DROP SPACE` on large keyspaces (tens of millions of
    /// keys): bounds memory and per-transaction work, and never blocks on a
    /// single giant delete.
    ///
    /// The default implementation materializes the full prefix via
    /// [`Self::scan_prefix`] (which copies values too) before deleting — fine
    /// for small ranges. Backends override this to read keys only and stream
    /// the deletes (see `RedbKVStore`). Returns the number of keys removed.
    async fn delete_prefix_chunked(&self, prefix: &[u8], chunk: usize) -> Result<usize> {
        let entries = self.scan_prefix(prefix).await?;
        let n = entries.len();
        let keys: Vec<Vec<u8>> = entries.into_iter().map(|(k, _)| k).collect();
        let chunk = chunk.max(1);
        for c in keys.chunks(chunk) {
            self.batch_delete(c.to_vec()).await?;
        }
        Ok(n)
    }

    /// Count entries under `prefix` **without copying values**. Used by edge
    /// degree aggregation (`MATCH (c)<-[:e]-() RETURN c, COUNT(*)`), where the
    /// answer is just how many edge keys share a `{space}:in-edge:{vid}:{etype}:`
    /// prefix — decoding 33M edge payloads to count them is the whole bottleneck.
    /// Default streams via `scan_prefix` (copies values); backends override for a
    /// keys-only range count.
    async fn count_prefix(&self, prefix: &[u8]) -> Result<u64> {
        Ok(self.scan_prefix(prefix).await?.len() as u64)
    }

    /// Count many prefixes in **one read transaction**, returning counts in input
    /// order. Edge-degree aggregation counts thousands of group nodes' prefixes;
    /// doing each in its own `count_prefix` pays the begin_read + open_table cost
    /// per node, which dominates. Batching amortizes it to once. Default loops
    /// `count_prefix`; backends override to share a single snapshot.
    async fn count_prefixes(&self, prefixes: Vec<Vec<u8>>) -> Result<Vec<u64>> {
        let mut out = Vec::with_capacity(prefixes.len());
        for p in &prefixes {
            out.push(self.count_prefix(p).await?);
        }
        Ok(out)
    }

    /// Atomically add signed deltas to i64-LE counter values in **one write
    /// transaction** (read-modify-write under a single writer → no lost updates).
    /// Missing keys start at 0; a resulting count ≤ 0 removes the key (degree
    /// counters never go negative). Used to maintain edge-degree counters on
    /// INSERT/DELETE EDGE. Default loops get/put (fine for in-memory backends).
    async fn add_counters(&self, deltas: Vec<(Vec<u8>, i64)>) -> Result<()> {
        for (key, delta) in deltas {
            let cur = self
                .get(&key)
                .await?
                .and_then(|b| b.get(..8).and_then(|s| s.try_into().ok()))
                .map(i64::from_le_bytes)
                .unwrap_or(0);
            let new = cur + delta;
            if new <= 0 {
                self.delete(&key).await?;
            } else {
                self.put(&key, &new.to_le_bytes()).await?;
            }
        }
        Ok(())
    }

    async fn scan_prefix(&self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>>;

    /// Scan the ordered key interval `[start, end)`, stopping after `limit`
    /// rows when a cap is provided. Secondary-index range lookups use this to
    /// seek directly to an encoded property boundary instead of materializing
    /// the whole index prefix.
    async fn scan_range(
        &self,
        start: &[u8],
        end: &[u8],
        limit: Option<usize>,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        if start >= end || limit == Some(0) {
            return Ok(Vec::new());
        }
        let common_len = start
            .iter()
            .zip(end.iter())
            .take_while(|(left, right)| left == right)
            .count();
        Ok(self
            .scan_prefix(&start[..common_len])
            .await?
            .into_iter()
            .filter(|(key, _)| key.as_slice() >= start && key.as_slice() < end)
            .take(limit.unwrap_or(usize::MAX))
            .collect())
    }

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
    /// When true, write commits use `Durability::None` (no per-commit fsync) for
    /// fast bulk loading, with a periodic `Immediate` checkpoint to bound crash
    /// loss. Set via `KVStoreOptions::use_fsync = false`. Default false (safe:
    /// every commit is `Immediate`).
    relaxed_durability: bool,
    /// Commit counter for periodic checkpointing under relaxed durability.
    commit_count: Arc<AtomicU64>,
}

/// Under relaxed (`Durability::None`) writes, force an `Immediate` (fsync)
/// commit every Nth write so a crash loses at most ~N commits' worth of data
/// (the data layer is idempotent/checkpointed, so re-load recovers it).
const CHECKPOINT_EVERY: u64 = 64;

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
            wtx.open_table(HISTORY_TABLE)?;
        }
        wtx.commit()?;

        info!(
            "redb opened at {:?} (cache_size={}MB)",
            file,
            opts.cache_size / (1024 * 1024)
        );
        Ok(RedbKVStore {
            db: Arc::new(db),
            relaxed_durability: !opts.use_fsync,
            commit_count: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Construct from an already-open database (used by tooling/tests).
    pub fn with_db(db: Arc<Database>) -> Self {
        RedbKVStore {
            db,
            relaxed_durability: false,
            commit_count: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Durability for the next write commit. `Immediate` (fsync) by default;
    /// under relaxed durability, `None` (no fsync) except every Nth commit which
    /// stays `Immediate` as a checkpoint.
    fn next_durability(&self) -> Durability {
        if !self.relaxed_durability {
            return Durability::Immediate;
        }
        let n = self.commit_count.fetch_add(1, Ordering::Relaxed);
        if n.is_multiple_of(CHECKPOINT_EVERY) {
            Durability::Immediate
        } else {
            Durability::None
        }
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
        let durability = self.next_durability();
        tokio::task::spawn_blocking(move || {
            let mut wtx = db.begin_write()?;
            wtx.set_durability(durability)?;
            {
                let mut table = wtx.open_table(KV_TABLE)?;
                table.insert(key.as_slice(), value.as_slice())?;
            }
            wtx.commit()?;
            Ok(())
        })
        .await?
    }

    async fn put_if_absent(&self, key: &[u8], value: &[u8]) -> Result<Option<Vec<u8>>> {
        let db = Arc::clone(&self.db);
        let key = key.to_vec();
        let value = value.to_vec();
        let durability = self.next_durability();
        tokio::task::spawn_blocking(move || {
            let mut wtx = db.begin_write()?;
            wtx.set_durability(durability)?;
            let existing = {
                let mut table = wtx.open_table(KV_TABLE)?;
                let existing = table
                    .get(key.as_slice())?
                    .map(|guard| guard.value().to_vec());
                if existing.is_none() {
                    table.insert(key.as_slice(), value.as_slice())?;
                }
                existing
            };
            wtx.commit()?;
            Ok(existing)
        })
        .await?
    }

    async fn delete(&self, key: &[u8]) -> Result<()> {
        let db = Arc::clone(&self.db);
        let key = key.to_vec();
        let durability = self.next_durability();
        tokio::task::spawn_blocking(move || {
            let mut wtx = db.begin_write()?;
            wtx.set_durability(durability)?;
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
        let durability = self.next_durability();
        tokio::task::spawn_blocking(move || {
            let mut wtx = db.begin_write()?;
            wtx.set_durability(durability)?;
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

    /// Delete all keys in a single transaction (one fsync). Missing keys are
    /// no-ops. Used by DROP SPACE to purge a space's key ranges efficiently.
    async fn batch_delete(&self, keys: Vec<Vec<u8>>) -> Result<()> {
        let db = Arc::clone(&self.db);
        let durability = self.next_durability();
        tokio::task::spawn_blocking(move || {
            let mut wtx = db.begin_write()?;
            wtx.set_durability(durability)?;
            {
                let mut table = wtx.open_table(KV_TABLE)?;
                for key in &keys {
                    table.remove(key.as_slice())?;
                }
            }
            wtx.commit()?;
            Ok(())
        })
        .await?
    }

    /// Commit an empty 2-phase (`Durability::Immediate`) transaction. This
    /// updates the allocator-state table and fsyncs, marking the store cleanly
    /// shut down so the next `open()` loads the allocator state directly instead
    /// of running a full repair (3 file scans). Cheap regardless of dataset size.
    async fn checkpoint(&self) -> Result<()> {
        let db = Arc::clone(&self.db);
        tokio::task::spawn_blocking(move || {
            let mut wtx = db.begin_write()?;
            wtx.set_durability(Durability::Immediate)?;
            wtx.commit()?;
            Ok(())
        })
        .await?
    }

    /// Keys-only chunked prefix delete. Each iteration opens a fresh read
    /// snapshot, collects up to `chunk` keys under `prefix` **without copying
    /// values** (DROP only needs keys — `scan_prefix` would copy every vertex
    /// blob), then deletes them in one write commit. Re-scanning from the start
    /// of `prefix` each round is O(log N) to reposition, and the just-deleted
    /// keys are gone from the next snapshot, so progress is monotonic. Memory
    /// stays bounded by `chunk` regardless of total keyspace size.
    async fn delete_prefix_chunked(&self, prefix: &[u8], chunk: usize) -> Result<usize> {
        let chunk = chunk.max(1);
        let mut total = 0usize;
        loop {
            let db = Arc::clone(&self.db);
            let prefix_v = prefix.to_vec();
            let keys: Vec<Vec<u8>> =
                tokio::task::spawn_blocking(move || -> Result<Vec<Vec<u8>>> {
                    let rtx = db.begin_read()?;
                    let table = rtx.open_table(KV_TABLE)?;
                    let mut out = Vec::with_capacity(chunk);
                    for entry in table.range(prefix_v.as_slice()..)? {
                        let (k, _) = entry?;
                        let kb = k.value();
                        if !kb.starts_with(&prefix_v) {
                            break;
                        }
                        out.push(kb.to_vec());
                        if out.len() >= chunk {
                            break;
                        }
                    }
                    Ok(out)
                })
                .await??;

            if keys.is_empty() {
                break;
            }
            total += keys.len();
            self.batch_delete(keys).await?;
            info!(
                prefix = %String::from_utf8_lossy(prefix),
                deleted = total,
                "delete_prefix_chunked progress"
            );
        }
        Ok(total)
    }

    /// Keys-only prefix count. Iterates `range(prefix..)` over the redb B-tree
    /// counting keys while they share `prefix`, never touching values. This is
    /// what makes edge-degree COUNT cheap: ~33M in-edge keys can be counted in
    /// seconds because no `EdgeData` payload is decoded.
    async fn count_prefix(&self, prefix: &[u8]) -> Result<u64> {
        let db = Arc::clone(&self.db);
        let prefix = prefix.to_vec();
        tokio::task::spawn_blocking(move || -> Result<u64> {
            let rtx = db.begin_read()?;
            let table = rtx.open_table(KV_TABLE)?;
            let mut n = 0u64;
            for entry in table.range(prefix.as_slice()..)? {
                let (k, _) = entry?;
                if !k.value().starts_with(&prefix) {
                    break;
                }
                n += 1;
            }
            Ok(n)
        })
        .await?
    }

    /// One read snapshot + one table open for the whole batch; each prefix is an
    /// independent keys-only range count. Amortizes the per-prefix transaction
    /// overhead that dominates edge-degree aggregation over thousands of nodes.
    async fn count_prefixes(&self, prefixes: Vec<Vec<u8>>) -> Result<Vec<u64>> {
        let db = Arc::clone(&self.db);
        tokio::task::spawn_blocking(move || -> Result<Vec<u64>> {
            let rtx = db.begin_read()?;
            let table = rtx.open_table(KV_TABLE)?;
            let mut out = Vec::with_capacity(prefixes.len());
            for prefix in &prefixes {
                let mut n = 0u64;
                for entry in table.range(prefix.as_slice()..)? {
                    let (k, _) = entry?;
                    if !k.value().starts_with(prefix) {
                        break;
                    }
                    n += 1;
                }
                out.push(n);
            }
            Ok(out)
        })
        .await?
    }

    /// Atomic counter increments in one write transaction: read each key, add the
    /// delta, and insert (or remove when ≤ 0) — all under redb's single writer, so
    /// concurrent INSERT/DELETE EDGE can't lose updates.
    async fn add_counters(&self, deltas: Vec<(Vec<u8>, i64)>) -> Result<()> {
        let db = Arc::clone(&self.db);
        let durability = self.next_durability();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut wtx = db.begin_write()?;
            wtx.set_durability(durability)?;
            {
                let mut table = wtx.open_table(KV_TABLE)?;
                for (key, delta) in &deltas {
                    let cur = table
                        .get(key.as_slice())?
                        .and_then(|g| g.value().get(..8).and_then(|s| s.try_into().ok()))
                        .map(i64::from_le_bytes)
                        .unwrap_or(0);
                    let new = cur + delta;
                    if new <= 0 {
                        table.remove(key.as_slice())?;
                    } else {
                        table.insert(key.as_slice(), new.to_le_bytes().as_slice())?;
                    }
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

    async fn scan_range(
        &self,
        start: &[u8],
        end: &[u8],
        limit: Option<usize>,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        if start >= end || limit == Some(0) {
            return Ok(Vec::new());
        }
        let db = Arc::clone(&self.db);
        let start = start.to_vec();
        let end = end.to_vec();
        let limit = limit.unwrap_or(usize::MAX);
        tokio::task::spawn_blocking(move || {
            let rtx = db.begin_read()?;
            let table = rtx.open_table(KV_TABLE)?;
            let mut results = Vec::new();
            for entry in table.range(start.as_slice()..end.as_slice())? {
                let (key, value) = entry?;
                results.push((key.value().to_vec(), value.value().to_vec()));
                if results.len() >= limit {
                    break;
                }
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

    async fn put_version(
        &self,
        entity_key: &[u8],
        valid_from: i64,
        valid_to: i64,
        tx: i64,
        value: &[u8],
    ) -> Result<()> {
        let db = Arc::clone(&self.db);
        let durability = self.next_durability();
        let key = history_key(entity_key, valid_from, tx);
        let val = encode_history_value(valid_to, value);
        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut wtx = db.begin_write()?;
            wtx.set_durability(durability)?;
            {
                let mut table = wtx.open_table(HISTORY_TABLE)?;
                table.insert(key.as_slice(), val.as_slice())?;
            }
            wtx.commit()?;
            Ok(())
        })
        .await?
    }

    async fn scan_history(&self, entity_key: &[u8]) -> Result<Vec<VersionRecord>> {
        let db = Arc::clone(&self.db);
        let prefix = history_prefix(entity_key);
        tokio::task::spawn_blocking(move || -> Result<Vec<VersionRecord>> {
            let rtx = db.begin_read()?;
            let table = rtx.open_table(HISTORY_TABLE)?;
            let mut out = Vec::new();
            for entry in table.range(prefix.as_slice()..)? {
                let (k, v) = entry?;
                let kb = k.value();
                if !kb.starts_with(&prefix) {
                    break;
                }
                let (Some((valid_from, tx)), Some((valid_to, value))) =
                    (parse_history_key_times(kb), decode_history_value(v.value()))
                else {
                    continue;
                };
                out.push(VersionRecord {
                    valid_from,
                    valid_to,
                    tx,
                    value,
                });
            }
            Ok(out)
        })
        .await?
    }

    async fn batch_put_version(
        &self,
        versions: Vec<(Vec<u8>, i64, i64, i64, Vec<u8>)>,
    ) -> Result<()> {
        if versions.is_empty() {
            return Ok(());
        }
        let db = Arc::clone(&self.db);
        let durability = self.next_durability();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut wtx = db.begin_write()?;
            wtx.set_durability(durability)?;
            {
                let mut table = wtx.open_table(HISTORY_TABLE)?;
                for (ek, vf, vt, tx, val) in &versions {
                    let key = history_key(ek, *vf, *tx);
                    let v = encode_history_value(*vt, val);
                    table.insert(key.as_slice(), v.as_slice())?;
                }
            }
            wtx.commit()?;
            Ok(())
        })
        .await?
    }

    async fn scan_history_entity_keys(&self, entity_prefix: &[u8]) -> Result<Vec<Vec<u8>>> {
        let db = Arc::clone(&self.db);
        let prefix = entity_prefix.to_vec();
        tokio::task::spawn_blocking(move || -> Result<Vec<Vec<u8>>> {
            let rtx = db.begin_read()?;
            let table = rtx.open_table(HISTORY_TABLE)?;
            let mut out: Vec<Vec<u8>> = Vec::new();
            for entry in table.range(prefix.as_slice()..)? {
                let (k, _v) = entry?;
                let kb = k.value();
                if !kb.starts_with(&prefix) {
                    break;
                }
                // history key = entity || 0x00 || 16B(times) — strip the suffix.
                if kb.len() < 17 || kb[kb.len() - 17] != 0x00 {
                    continue;
                }
                let entity = &kb[..kb.len() - 17];
                // One entity's versions are key-consecutive → last-check dedupes.
                if out.last().map(|e| e.as_slice()) != Some(entity) {
                    out.push(entity.to_vec());
                }
            }
            Ok(out)
        })
        .await?
    }

    /// Truly atomic dual-write: KV_TABLE puts/deletes and HISTORY_TABLE appends
    /// commit in one redb write transaction (T-트랙 v1.1).
    async fn batch_apply(
        &self,
        puts: Vec<(Vec<u8>, Vec<u8>)>,
        deletes: Vec<Vec<u8>>,
        versions: Vec<(Vec<u8>, i64, i64, i64, Vec<u8>)>,
    ) -> Result<()> {
        if puts.is_empty() && deletes.is_empty() && versions.is_empty() {
            return Ok(());
        }
        let db = Arc::clone(&self.db);
        let durability = self.next_durability();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut wtx = db.begin_write()?;
            wtx.set_durability(durability)?;
            {
                let mut kv = wtx.open_table(KV_TABLE)?;
                for (k, v) in &puts {
                    kv.insert(k.as_slice(), v.as_slice())?;
                }
                for k in &deletes {
                    kv.remove(k.as_slice())?;
                }
                let mut hist = wtx.open_table(HISTORY_TABLE)?;
                for (ek, vf, vt, tx, val) in &versions {
                    let key = history_key(ek, *vf, *tx);
                    let v = encode_history_value(*vt, val);
                    hist.insert(key.as_slice(), v.as_slice())?;
                }
            }
            wtx.commit()?;
            Ok(())
        })
        .await?
    }

    /// Seek-based bitemporal resolution. History keys sort newest-first
    /// (desc valid_from, desc tx), so a range starting at `(valid_at, tx_at)`
    /// yields candidates in exactly the "greatest (valid_from, tx)" preference
    /// order — the first row whose tx and interval qualify is the answer.
    /// O(seek + skipped rows) instead of the trait default's O(all versions).
    async fn get_as_of(
        &self,
        entity_key: &[u8],
        valid_at: i64,
        tx_at: i64,
    ) -> Result<Option<Vec<u8>>> {
        let db = Arc::clone(&self.db);
        let prefix = history_prefix(entity_key);
        let start = history_key(entity_key, valid_at, tx_at);
        tokio::task::spawn_blocking(move || -> Result<Option<Vec<u8>>> {
            let rtx = db.begin_read()?;
            let table = rtx.open_table(HISTORY_TABLE)?;
            for entry in table.range(start.as_slice()..)? {
                let (k, v) = entry?;
                let kb = k.value();
                if !kb.starts_with(&prefix) {
                    break;
                }
                // Ordering guarantees valid_from <= valid_at within the range;
                // tx and valid_to must still be checked per row (an older
                // valid_from may carry a later tx — e.g. a correction).
                let (Some((_vf, tx)), Some((valid_to, value))) =
                    (parse_history_key_times(kb), decode_history_value(v.value()))
                else {
                    continue;
                };
                if tx <= tx_at && valid_at < valid_to {
                    return Ok(Some(value));
                }
            }
            Ok(None)
        })
        .await?
    }
}

/// In-memory KVStore for testing
pub struct MemoryKVStore {
    data: Arc<tokio::sync::RwLock<std::collections::BTreeMap<Vec<u8>, Vec<u8>>>>,
    /// Separate ordered map for version history — mirrors RedbKVStore's
    /// `HISTORY_TABLE` physical isolation from the current-view keyspace.
    history: Arc<tokio::sync::RwLock<std::collections::BTreeMap<Vec<u8>, Vec<u8>>>>,
}

impl MemoryKVStore {
    pub fn new() -> Self {
        MemoryKVStore {
            data: Arc::new(tokio::sync::RwLock::new(std::collections::BTreeMap::new())),
            history: Arc::new(tokio::sync::RwLock::new(std::collections::BTreeMap::new())),
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

    async fn put_if_absent(&self, key: &[u8], value: &[u8]) -> Result<Option<Vec<u8>>> {
        let mut data = self.data.write().await;
        if let Some(existing) = data.get(key) {
            return Ok(Some(existing.clone()));
        }
        data.insert(key.to_vec(), value.to_vec());
        Ok(None)
    }

    async fn put_version(
        &self,
        entity_key: &[u8],
        valid_from: i64,
        valid_to: i64,
        tx: i64,
        value: &[u8],
    ) -> Result<()> {
        let mut h = self.history.write().await;
        h.insert(
            history_key(entity_key, valid_from, tx),
            encode_history_value(valid_to, value),
        );
        Ok(())
    }

    async fn batch_put_version(
        &self,
        versions: Vec<(Vec<u8>, i64, i64, i64, Vec<u8>)>,
    ) -> Result<()> {
        let mut h = self.history.write().await;
        for (ek, vf, vt, tx, val) in &versions {
            h.insert(history_key(ek, *vf, *tx), encode_history_value(*vt, val));
        }
        Ok(())
    }

    async fn scan_history_entity_keys(&self, entity_prefix: &[u8]) -> Result<Vec<Vec<u8>>> {
        let h = self.history.read().await;
        let mut out: Vec<Vec<u8>> = Vec::new();
        for (k, _v) in h.range(entity_prefix.to_vec()..) {
            if !k.starts_with(entity_prefix) {
                break;
            }
            if k.len() < 17 || k[k.len() - 17] != 0x00 {
                continue;
            }
            let entity = &k[..k.len() - 17];
            if out.last().map(|e| e.as_slice()) != Some(entity) {
                out.push(entity.to_vec());
            }
        }
        Ok(out)
    }

    /// Atomic dual-write: both maps' write locks are held for the whole apply,
    /// so no reader observes the current view without its history (or vice versa).
    async fn batch_apply(
        &self,
        puts: Vec<(Vec<u8>, Vec<u8>)>,
        deletes: Vec<Vec<u8>>,
        versions: Vec<(Vec<u8>, i64, i64, i64, Vec<u8>)>,
    ) -> Result<()> {
        let mut data = self.data.write().await;
        let mut h = self.history.write().await;
        for (k, v) in puts {
            data.insert(k, v);
        }
        for k in &deletes {
            data.remove(k);
        }
        for (ek, vf, vt, tx, val) in &versions {
            h.insert(history_key(ek, *vf, *tx), encode_history_value(*vt, val));
        }
        Ok(())
    }

    /// Seek-based bitemporal resolution — same ordering argument as the
    /// `RedbKVStore` override (keys sort newest-first).
    async fn get_as_of(
        &self,
        entity_key: &[u8],
        valid_at: i64,
        tx_at: i64,
    ) -> Result<Option<Vec<u8>>> {
        let prefix = history_prefix(entity_key);
        let start = history_key(entity_key, valid_at, tx_at);
        let h = self.history.read().await;
        for (k, v) in h.range(start..) {
            if !k.starts_with(&prefix) {
                break;
            }
            let (Some((_vf, tx)), Some((valid_to, value))) =
                (parse_history_key_times(k), decode_history_value(v))
            else {
                continue;
            };
            if tx <= tx_at && valid_at < valid_to {
                return Ok(Some(value));
            }
        }
        Ok(None)
    }

    async fn scan_history(&self, entity_key: &[u8]) -> Result<Vec<VersionRecord>> {
        let prefix = history_prefix(entity_key);
        let h = self.history.read().await;
        let mut out = Vec::new();
        for (k, v) in h.range(prefix.clone()..) {
            if !k.starts_with(&prefix) {
                break;
            }
            let (Some((valid_from, tx)), Some((valid_to, value))) =
                (parse_history_key_times(k), decode_history_value(v))
            else {
                continue;
            };
            out.push(VersionRecord {
                valid_from,
                valid_to,
                tx,
                value,
            });
        }
        Ok(out)
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

    async fn batch_delete(&self, keys: Vec<Vec<u8>>) -> Result<()> {
        let mut data = self.data.write().await;
        for key in &keys {
            data.remove(key);
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

    async fn scan_range(
        &self,
        start: &[u8],
        end: &[u8],
        limit: Option<usize>,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        if start >= end || limit == Some(0) {
            return Ok(Vec::new());
        }
        let data = self.data.read().await;
        Ok(data
            .range(start.to_vec()..end.to_vec())
            .take(limit.unwrap_or(usize::MAX))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect())
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
        assert!(opts.use_fsync); // Immediate durability (fsync) by default
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
    async fn test_memory_put_if_absent_preserves_the_winner() {
        let store = MemoryKVStore::new();
        assert_eq!(store.put_if_absent(b"k", b"v1").await.unwrap(), None);
        assert_eq!(
            store.put_if_absent(b"k", b"v2").await.unwrap(),
            Some(b"v1".to_vec())
        );
        assert_eq!(store.get(b"k").await.unwrap(), Some(b"v1".to_vec()));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_memory_concurrent_put_if_absent_has_exactly_one_winner() {
        let store = Arc::new(MemoryKVStore::new());
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let attempt = |value: &'static [u8]| {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            tokio::spawn(async move {
                barrier.wait().await;
                store.put_if_absent(b"claim", value).await.unwrap()
            })
        };

        let (first, second) = tokio::join!(attempt(b"first"), attempt(b"second"));
        let outcomes = [first.unwrap(), second.unwrap()];
        assert_eq!(outcomes.iter().filter(|value| value.is_none()).count(), 1);
        let stored = store.get(b"claim").await.unwrap().unwrap();
        assert!(stored == b"first" || stored == b"second");
        assert!(outcomes
            .iter()
            .filter_map(|value| value.as_ref())
            .all(|existing| existing == &stored));
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
    async fn test_memory_scan_range_respects_bounds_and_limit() {
        let store = MemoryKVStore::new();
        for key in [b"a", b"b", b"c", b"d", b"e"] {
            store.put(key, key).await.unwrap();
        }
        let results = store.scan_range(b"b", b"e", Some(2)).await.unwrap();
        let keys: Vec<Vec<u8>> = results.into_iter().map(|(key, _)| key).collect();
        assert_eq!(keys, vec![b"b".to_vec(), b"c".to_vec()]);
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
    async fn test_redb_kvstore_put_if_absent_preserves_the_winner() {
        let dir = tempdir().unwrap();
        let store = RedbKVStore::open(dir.path(), KVStoreOptions::default()).unwrap();
        assert_eq!(store.put_if_absent(b"k", b"v1").await.unwrap(), None);
        assert_eq!(
            store.put_if_absent(b"k", b"v2").await.unwrap(),
            Some(b"v1".to_vec())
        );
        assert_eq!(store.get(b"k").await.unwrap(), Some(b"v1".to_vec()));
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
    async fn test_redb_kvstore_scan_range_respects_bounds_and_limit() {
        let dir = tempdir().unwrap();
        let store = RedbKVStore::open(dir.path(), KVStoreOptions::default()).unwrap();
        for key in [b"a", b"b", b"c", b"d", b"e"] {
            store.put(key, key).await.unwrap();
        }
        let results = store.scan_range(b"b", b"e", Some(2)).await.unwrap();
        let keys: Vec<Vec<u8>> = results.into_iter().map(|(key, _)| key).collect();
        assert_eq!(keys, vec![b"b".to_vec(), b"c".to_vec()]);
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_delete_prefix_chunked_removes_only_prefix() {
        let dir = tempdir().unwrap();
        let store = RedbKVStore::open(dir.path(), KVStoreOptions::default()).unwrap();
        // 25 keys under "a:", 5 under "b:" — chunk 10 forces multiple commits
        // (the large-keyspace path) and a sibling prefix that must survive.
        for i in 0..25 {
            store
                .put(format!("a:{i:02}").as_bytes(), b"val")
                .await
                .unwrap();
        }
        for i in 0..5 {
            store
                .put(format!("b:{i:02}").as_bytes(), b"val")
                .await
                .unwrap();
        }
        let removed = store.delete_prefix_chunked(b"a:", 10).await.unwrap();
        assert_eq!(removed, 25);
        assert!(store.scan_prefix(b"a:").await.unwrap().is_empty());
        assert_eq!(store.scan_prefix(b"b:").await.unwrap().len(), 5);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_checkpoint_succeeds_under_relaxed_and_preserves_data() {
        // Under relaxed durability (the bulk-load / risky path), checkpoint must
        // commit cleanly and leave data intact. This is what graceful shutdown
        // and the loader call to avoid the next open()'s full repair.
        let dir = tempdir().unwrap();
        let store = RedbKVStore::open(
            dir.path(),
            KVStoreOptions {
                use_fsync: false,
                ..KVStoreOptions::default()
            },
        )
        .unwrap();
        store.put(b"k", b"v").await.unwrap();
        store.checkpoint().await.unwrap();
        assert_eq!(store.get(b"k").await.unwrap(), Some(b"v".to_vec()));
        // Reopening after a checkpoint must still see the data.
        drop(store);
        let reopened = RedbKVStore::open(dir.path(), KVStoreOptions::default()).unwrap();
        assert_eq!(reopened.get(b"k").await.unwrap(), Some(b"v".to_vec()));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_delete_prefix_chunked_absent_prefix_is_noop() {
        let dir = tempdir().unwrap();
        let store = RedbKVStore::open(dir.path(), KVStoreOptions::default()).unwrap();
        store.put(b"x:1", b"v").await.unwrap();
        assert_eq!(
            store.delete_prefix_chunked(b"absent:", 100).await.unwrap(),
            0
        );
        assert_eq!(store.scan_prefix(b"x:").await.unwrap().len(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_count_prefix_counts_only_matching_prefix() {
        let dir = tempdir().unwrap();
        let store = RedbKVStore::open(dir.path(), KVStoreOptions::default()).unwrap();
        for i in 0..7 {
            store.put(format!("a:{i}").as_bytes(), b"v").await.unwrap();
        }
        for i in 0..3 {
            store.put(format!("b:{i}").as_bytes(), b"v").await.unwrap();
        }
        // Counts keys under the prefix without bleeding into a sibling prefix.
        assert_eq!(store.count_prefix(b"a:").await.unwrap(), 7);
        assert_eq!(store.count_prefix(b"b:").await.unwrap(), 3);
        assert_eq!(store.count_prefix(b"absent:").await.unwrap(), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_count_prefixes_batches_in_one_snapshot() {
        let dir = tempdir().unwrap();
        let store = RedbKVStore::open(dir.path(), KVStoreOptions::default()).unwrap();
        for i in 0..7 {
            store.put(format!("a:{i}").as_bytes(), b"v").await.unwrap();
        }
        for i in 0..3 {
            store.put(format!("b:{i}").as_bytes(), b"v").await.unwrap();
        }
        // Counts returned in input order, matching individual count_prefix.
        let got = store
            .count_prefixes(vec![b"a:".to_vec(), b"b:".to_vec(), b"absent:".to_vec()])
            .await
            .unwrap();
        assert_eq!(got, vec![7, 3, 0]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_add_counters_increment_decrement_and_remove() {
        let dir = tempdir().unwrap();
        let store = RedbKVStore::open(dir.path(), KVStoreOptions::default()).unwrap();
        let dec = |b: Option<Vec<u8>>| -> i64 {
            b.and_then(|x| x.get(..8).and_then(|s| s.try_into().ok()))
                .map(i64::from_le_bytes)
                .unwrap_or(0)
        };
        // Fresh keys start at 0.
        store
            .add_counters(vec![(b"a".to_vec(), 5), (b"b".to_vec(), 3)])
            .await
            .unwrap();
        assert_eq!(dec(store.get(b"a").await.unwrap()), 5);
        assert_eq!(dec(store.get(b"b").await.unwrap()), 3);
        // Decrement; b hits 0 and is removed (counters never go negative).
        store
            .add_counters(vec![(b"a".to_vec(), -2), (b"b".to_vec(), -3)])
            .await
            .unwrap();
        assert_eq!(dec(store.get(b"a").await.unwrap()), 3);
        assert!(store.get(b"b").await.unwrap().is_none());
    }
}

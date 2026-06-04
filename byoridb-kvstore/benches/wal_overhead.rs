// Benchmarks isolating two suspected bottlenecks in byoridb-kvstore:
//
//   1) Per-write flush in WAL::write_entry — each put/delete calls
//      BufWriter::flush(), which is OS-buffer-only (not fsync).
//   2) Double WAL — WalKVStore writes its own WAL *and* RocksDB writes
//      its internal WAL on every put.
//
// Methodology:
//   - One DB instance per benchmark group (setup outside iter).
//   - Each iteration writes to a fresh, monotonic key so we always
//     hit memtable, never overwrite the same row.
//   - Tokio multi-thread runtime is required because both backends use
//     `block_in_place` for single-key ops.
//
// Comparisons:
//   * rocksdb_put vs wal_put              -> double-WAL overhead (item 2)
//   * wal_serial_100 vs wal_batch_100     -> per-write flush overhead
//                                            (item 1; double-WAL cost
//                                            cancels on both sides)

use byoridb_kvstore::{KVStore, KVStoreOptions, RocksdbKVStore, WalKVStore};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::runtime::Runtime;

const VALUE_SIZE: usize = 256;
const BATCH_SIZE: usize = 100;

fn make_runtime() -> Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("multi-thread runtime")
}

fn make_key(counter: &AtomicU64) -> Vec<u8> {
    let n = counter.fetch_add(1, Ordering::Relaxed);
    let mut k = Vec::with_capacity(16);
    k.extend_from_slice(b"k:");
    k.extend_from_slice(&n.to_be_bytes());
    k
}

// ---------------------------------------------------------------------
// (2) Double-WAL overhead: single put on RocksdbKVStore vs WalKVStore
// ---------------------------------------------------------------------

fn bench_single_put_rocksdb_vs_wal(c: &mut Criterion) {
    let rt = make_runtime();
    let value = vec![0xABu8; VALUE_SIZE];

    let mut group = c.benchmark_group("single_put");
    group.throughput(Throughput::Bytes(VALUE_SIZE as u64));
    // Disk I/O dominates — keep the budget reasonable but stable.
    group.sample_size(50);

    // --- Rocksdb only (RocksDB internal WAL ON, no external WAL) ---
    {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(
            RocksdbKVStore::open(dir.path(), KVStoreOptions::default()).expect("rocksdb open"),
        );
        let counter = AtomicU64::new(0);

        group.bench_function(BenchmarkId::new("backend", "rocksdb"), |b| {
            b.iter(|| {
                let key = make_key(&counter);
                rt.block_on(async {
                    store.put(&key, &value).await.unwrap();
                });
            })
        });
    }

    // --- WalKVStore (external WAL + RocksDB internal WAL) ---
    {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(
            WalKVStore::open(dir.path(), KVStoreOptions::default()).expect("wal kvstore open"),
        );
        let counter = AtomicU64::new(0);

        group.bench_function(BenchmarkId::new("backend", "wal_kvstore"), |b| {
            b.iter(|| {
                let key = make_key(&counter);
                rt.block_on(async {
                    store.put(&key, &value).await.unwrap();
                });
            })
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------
// (1) Per-write flush overhead: 100 serial puts vs single batch_put(100)
//     — both go through WalKVStore so the double-WAL cost is identical
//       on each side. The remaining delta is dominated by flush count
//       (100 flushes vs 1) on the external WAL.
// ---------------------------------------------------------------------

fn bench_serial_vs_batch_wal(c: &mut Criterion) {
    let rt = make_runtime();
    let value = vec![0xABu8; VALUE_SIZE];

    let mut group = c.benchmark_group("hundred_puts");
    group.throughput(Throughput::Bytes((VALUE_SIZE * BATCH_SIZE) as u64));
    group.sample_size(30);

    // --- Serial: BATCH_SIZE individual puts (BATCH_SIZE WAL flushes) ---
    {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(
            WalKVStore::open(dir.path(), KVStoreOptions::default()).expect("wal kvstore open"),
        );
        let counter = AtomicU64::new(0);

        group.bench_function(BenchmarkId::new("mode", "serial"), |b| {
            b.iter(|| {
                rt.block_on(async {
                    for _ in 0..BATCH_SIZE {
                        let key = make_key(&counter);
                        store.put(&key, &value).await.unwrap();
                    }
                });
            })
        });
    }

    // --- Batch: one batch_put(BATCH_SIZE) (1 WAL flush) ---
    {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(
            WalKVStore::open(dir.path(), KVStoreOptions::default()).expect("wal kvstore open"),
        );
        let counter = AtomicU64::new(0);

        group.bench_function(BenchmarkId::new("mode", "batch"), |b| {
            b.iter(|| {
                let pairs: Vec<(Vec<u8>, Vec<u8>)> = (0..BATCH_SIZE)
                    .map(|_| (make_key(&counter), value.clone()))
                    .collect();
                rt.block_on(async {
                    store.batch_put(pairs).await.unwrap();
                });
            })
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------
// (2 cont.) Double-WAL on the batch path:
//     RocksdbKVStore::batch_put(100) vs WalKVStore::batch_put(100).
//     Removes per-write-flush noise so the gap is purely external-WAL
//     fixed cost (one append_batch + one flush).
// ---------------------------------------------------------------------

fn bench_batch_put_rocksdb_vs_wal(c: &mut Criterion) {
    let rt = make_runtime();
    let value = vec![0xABu8; VALUE_SIZE];

    let mut group = c.benchmark_group("batch_put_100");
    group.throughput(Throughput::Bytes((VALUE_SIZE * BATCH_SIZE) as u64));
    group.sample_size(30);

    {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(
            RocksdbKVStore::open(dir.path(), KVStoreOptions::default()).expect("rocksdb open"),
        );
        let counter = AtomicU64::new(0);

        group.bench_function(BenchmarkId::new("backend", "rocksdb"), |b| {
            b.iter(|| {
                let pairs: Vec<(Vec<u8>, Vec<u8>)> = (0..BATCH_SIZE)
                    .map(|_| (make_key(&counter), value.clone()))
                    .collect();
                rt.block_on(async {
                    store.batch_put(pairs).await.unwrap();
                });
            })
        });
    }

    {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(
            WalKVStore::open(dir.path(), KVStoreOptions::default()).expect("wal kvstore open"),
        );
        let counter = AtomicU64::new(0);

        group.bench_function(BenchmarkId::new("backend", "wal_kvstore"), |b| {
            b.iter(|| {
                let pairs: Vec<(Vec<u8>, Vec<u8>)> = (0..BATCH_SIZE)
                    .map(|_| (make_key(&counter), value.clone()))
                    .collect();
                rt.block_on(async {
                    store.batch_put(pairs).await.unwrap();
                });
            })
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_single_put_rocksdb_vs_wal,
    bench_serial_vs_batch_wal,
    bench_batch_put_rocksdb_vs_wal,
);
criterion_main!(benches);

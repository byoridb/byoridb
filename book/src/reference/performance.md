# Performance

[한국어](../ko/reference/performance.html)

ByoriDB does not publish a stable QPS or latency table. Results depend on the
dataset, query shape, redb file, durability, cache warmth, filesystem, and
hardware. Any number used for capacity planning should come from a reproducible
run against the intended build and workload.

## Benchmark tools in the repository

### Criterion microbenchmarks

`benches/benchmark.rs` measures object creation, parsing, plan building,
serialization, filters, and arena allocation:

```bash
cargo bench --locked -p byoridb --bench benchmark
```

### In-process end-to-end benchmarks

`benches/e2e_benchmark.rs` creates temporary redb databases and exercises batch
inserts, `FETCH`, `GO`, `LOOKUP`, and complete query-service calls:

```bash
cargo bench --locked -p byoridb --bench e2e_benchmark
```

These runs do not include network latency or a persistent production-sized
database. Preserve Criterion output, commit SHA, Rust version, operating system,
CPU, memory, filesystem, redb file size, cache settings, and durability with any
reported result.

### gRPC load generator

The client package contains a simple concurrent load generator:

```bash
export BYORIDB_USER=root
export BYORIDB_PASSWORD='same-secret-used-to-start-the-server'

cargo run --locked --release -p byoridb-client --bin load_test -- \
  --address http://127.0.0.1:9669 \
  --concurrency 20 \
  --duration 30 \
  --setup 'USE example' \
  --query 'FETCH PROP ON person 1'
```

It reports request count, errors, and average/per-second QPS. It is not a
latency-distribution benchmark and does not provision test data for you.

## Inspect a query before tuning

`EXPLAIN` derives a logical operator tree and reports the selected access path
without executing the statement:

```sql
EXPLAIN MATCH (p:person) WHERE p.name == "Alice" RETURN p;
```

The `access` column distinguishes named indexes, the tag-VID index, point
lookups, edge-prefix/reverse-edge access, and a full scan.

`PROFILE` executes the query and overlays observations at instrumented points:

```sql
PROFILE GO FROM 1 OVER knows YIELD dst(edge);
PROFILE MATCH (p:person) RETURN count(p);
```

Its columns are `id`, `operator`, `rows`, `time(us)`, `access`, and `detail`.
The executor is imperative rather than a Volcano iterator tree, so not every
operator has independently attributable timing. Treat missing timing as
unmeasured, not zero.

## Query practices

### Prefer point access when the VID is known

```sql
FETCH PROP ON person 42;
```

`FETCH` builds exact current-view keys and uses a batch get for multiple VIDs.
A general `MATCH` has more planning and scan/expansion work. Batches containing
hundreds of IDs and their response-size/performance bounds still need the
LDBC-scale regression coverage tracked in
[issue #10](https://github.com/byoridb/byoridb/issues/10).

### Create and use secondary indexes

```sql
CREATE TAG INDEX person_name_idx ON person(name);
LOOKUP ON person WHERE person.name == "Alice";
```

Use `EXPLAIN` to confirm the access path. Label-only MATCH can use the
automatically maintained tag-VID index; older data created before that index
entry existed may fall back to a full scan until backfilled/reloaded.

Only equality `LOOKUP` predicates currently route through a tag secondary
index. Range predicates (`>`, `>=`, `<`, `<=`) use a bounded full scan even when
an index exists; range index scans remain open in
[issue #1](https://github.com/byoridb/byoridb/issues/1).

### Bound result sets

```sql
MATCH (p:person) RETURN p LIMIT 100;
```

Avoid returning full vertex payloads when only an aggregate or a few
properties are needed. Result materialization is memory-bound and the
application guard is an estimate, not a substitute for a bounded query.

### Batch writes

```sql
INSERT VERTEX person(name, age) VALUES
  1:("Alice", 30),
  2:("Bob", 25),
  3:("Carol", 28);
```

One multi-row statement lets the executor commit a redb batch instead of one
transaction per statement. The current entity writes and matching temporal
history versions passed to `batch_apply` are committed together.

### Use the correct traversal direction

Outgoing traversal scans an edge prefix bounded by source VID. Incoming and
undirected traversal use the maintained reverse-edge index rather than a full
edge scan. Large degrees and variable-length paths can still expand rapidly;
use narrow edge types, bounded step ranges, filters, and limits.

## Runtime guards

The default execution context applies:

| Guard | Default | Configuration |
|---|---:|---|
| Estimated result-memory budget | 1024 MiB | `BYORIDB_MAX_MEMORY_MB`; `0` disables |
| Rows returned by guarded prefix scans | 100,000 | `BYORIDB_MAX_SCAN_LIMIT`; `0` disables |
| Traversal/materialization visited nodes | 100,000 | internal execution default |
| Maximum GO/MATCH path steps | 20 | internal execution default |
| Maximum enumerated shortest paths | 1,024 | internal execution default |

Some traversal algorithms warn and return partial/truncated results at a cap,
while excessive step ranges return an error. Raising or disabling a guard can
turn an incomplete analytical query into an out-of-memory process failure;
change one at a time and observe process/PVC metrics.

The `timeout_ms` field in `ExecutionConfig` is not currently enforced as a
general server-side query timeout. Apply an external timeout with care: an HTTP
client timing out does not necessarily cancel work already running on the
server. Use the authenticated active-query diagnostic to inspect it.

## redb tuning

### Page cache

```bash
export BYORIDB_CACHE_SIZE_MB=4096
```

The default is 256 MiB. Increase it only when the process has headroom for the
cache plus query materialization, indexes, and the operating system. Measure
cold and warm runs separately.

### Durability

Normal serving uses Immediate durability with fsync per write transaction.
`BYORIDB_DURABILITY=none`, `relaxed`, or `eventual` enables a faster bulk-load
mode in which recent commits can be lost on a crash. Use that only for an
idempotent, reloadable import and return to the default afterward.

There are no supported `block_cache_size`, `write_buffer_size`, compression, or
compaction configuration keys. Those are LSM/RocksDB concepts, not ByoriDB's
redb tuning surface.

## Temporal-read costs

- Current reads stay in the `kv` table and do not scan history.
- A vertex `AS OF` read uses the ordered history key to seek near the requested
  `(valid_at, transaction_at)` point, then checks qualifying versions.
- An edge `AS OF` read first enumerates historical entity keys under the source
  and edge-type prefix, then resolves each candidate. Its cost grows with the
  number of historical edge identities under that prefix.
- History is asserted-fact-only. Inference and ordinary traversals use the
  current view.

Test retention growth and historical edge workloads explicitly; no automatic
history pruning policy is currently implemented.

## Other boundaries

- redb serializes write transactions, so adding client concurrency does not
  create parallel disk writers.
- gRPC gzip/zstd support reduces some network payloads but is not storage
  compression.
- Persisted HNSW is used only above the executor's vector-count threshold;
  smaller sets use exact flat cosine search.
- The checked-in multi-node-looking Compose configuration is not a distributed
  performance topology. Benchmark it only as independent databases.

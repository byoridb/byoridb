# Performance

[한국어](../ko/reference/performance.html)

ByoriDB does not publish a stable QPS or latency table. Results depend on the
dataset, query shape, redb file, durability, cache warmth, filesystem, and
hardware. Any number used for capacity planning should come from a reproducible
run against the intended build and workload.

## Agent task outcomes

The [agent-first design](../architecture/agent-first.html) makes successful
agent tasks the product-level measure. EVAL-1 has deterministic contract fixtures; this page does not
report agent benchmark results. A future run must pin model/prompt/tool/server
versions, fixtures, budgets, and repetitions; verify answers, evidence, and
final state before comparing calls, tokens, latency, or cost. Include failed
runs, duplicate writes, unauthorized effects, and recovery after interruption.
Keep engine-only measurements separate from the full agent workflow.

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

Standalone `LOOKUP` supports equality and eligible ordered ranges (`>`, `>=`,
`<`, `<=`) through single-field tag and edge indexes. Edge `UPDATE` maintains
its index entries together with the edge changes.

- Tag range access requires a matching Boolean or integer schema and literal.
  Float/Double properties use predicate scans because stored integer and float
  values occupy different index key domains.
- The edge range helper accepts Boolean, integer and nonzero, non-NaN float
  boundaries. Schema-aware query selection sends
  Float/Double edge properties to predicate scans so mixed integer/float key
  domains cannot omit matches; `EXPLAIN` agrees with execution. String keys do
  not provide lexical ranges, and unsupported boundaries also use predicate scans.
- Ordered `LOOKUP` ranges are rejected in distributed execution. `!=` is not
  an ordered range fast path.

Check the actual `EXPLAIN`/`PROFILE` access path for each predicate and data type;
an index's existence does not guarantee that a query uses it.

### Bound result sets

```sql
MATCH (p:person) RETURN p LIMIT 100;
```

Avoid returning full vertex payloads when only an aggregate or a few
properties are needed. Result materialization is memory-bound and the
application guard is an estimate, not a substitute for a bounded query.

### MATCH count and aggregate execution

**Current MATCH contract:**
unsafe indexed-group and degree-counter shortcuts have been removed. Eligible
plain single-node `COUNT(*)` still streams validated candidates, applying a raw
cap+1 probe per prefix (`0` unbounded, exact cap permitted) before its result
window. Other grouped/edge/count queries use normal validated matching. They may
be slower, consume more memory or reach a scan/traversal budget where an old
shortcut returned a number; remeasure them instead of retaining old timings.

The raw storage grouping helper now checks key encodings, but it is not the live
query-count implementation and has no query budget of its own. It must not serve
as evidence that a query is complete or bounded. Result `LIMIT/OFFSET` applies
after aggregate evaluation and cannot hide overflow. Integer SUM preserves its
integer result type and exact arithmetic; floating SUM/AVG reject nonfinite
results. This does not establish complete mixed-type MIN/MAX ordering.

Canonical typed group keys sort map entries, use graph-entity identity and
normalize floating signed zero while keeping integer/float groups distinct.
Nonfinite and Path/Set/DataSet/Geography group keys are rejected. ORDER BY resolves
unambiguous aliases or exact projected expressions before scanning; unsupported
references fail instead of silently skipping a sort. Aggregate output uses the
shared estimated row-size budget, not a bound on all intermediate allocations.

Projection, WHERE, grouping and aggregates propagate relevant storage/decode/VID
errors; missing values remain distinct. Strict vertex decoding is shared by
LOOKUP, MATCH and ontology reads, while MATCH edge/intermediate identity checks
apply to decoded records. Bound required/OPTIONAL starts revalidate labels and
properties, and undirected self-loops deduplicate by full edge identity while
preserving parallel ranks/opposite edges. Required/OPTIONAL bound destinations and
repeated node/edge identities are preserved; encountered node/edge binding-kind collisions fail,
and newly introduced OPTIONAL nulls cannot rebind later. Reverse lookup and
`EXPLAIN` use the outgoing/no-range restriction. Partial indexes and remaining
MATCH contracts still need separate acceptance. This does not establish a global
query-work bound. Distributed LOOKUP and inference recovery have the separate
contracts described below and in [data queries](../guide/ngql/dql.md).

### Batch writes

```sql
INSERT VERTEX person(name, age) VALUES
  1:("Alice", 30),
  2:("Bob", 25),
  3:("Carol", 28);
```

One DML statement stages current entities, asserted history, secondary/reverse
indexes, vectors, counters and sameAs changes before one atomic Memory/redb
`batch_apply`. A preparation error or cancellation leaves the base store
unchanged. `BYORIDB_MAX_MEMORY_MB` also bounds the staging write set separately
from result rows; it does not bound all temporary allocations.

Inference closure is completed after the assertion batch, with a durable dirty
marker blocking graph reads and further writes until completion or administrator
repair. An inference error may therefore follow a committed assertion. Compound
statements commit clause by clause, with no transaction-wide rollback. Embedded
custom KV backends need atomic `batch_apply` for the same guarantee.

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
| Raw candidates per guarded prefix/index-partition scan | 100,000 | `BYORIDB_MAX_SCAN_LIMIT`; `0` disables |
| Traversal work budget (`max_traversal_nodes`) | 100,000 | internal execution default; `0` disables |
| Maximum GO/MATCH path steps | 20 | internal execution default |
| Enumerated shortest paths (`max_find_paths`) | 1,024 | internal execution default; `0` disables |

**Current source contract:** local LOOKUP probes
one extra raw candidate per full-prefix or index-partition scan. Exactly at cap
can succeed; cap+1 fails, even if a small `LIMIT/OFFSET` would discard the extra
candidate. This per-scan budget does not bound total query work or memory.
Distributed indexed LOOKUP applies this budget per partition with query-bound
keyset pages and remote payload validation; it has no cross-page snapshot. Relevant storage/schema/decode
errors and malformed edge-index keys fail rather than becoming partial success.
Shared LOOKUP/FIND edge decoding validates JSON, structural member types and
explicit key/payload identities; LOOKUP additionally validates vertex records.
Legacy omitted VID/rank/properties remain supported. Only decoded records are
validated: LIMIT can stop later decoding and FIND can skip unrelated types. This
is neither exhaustive corruption detection nor FIND vertex-record validation.

FIND charges expanded vertices or, for hop-bounded weighted paths, vertex/hop
states. It does not bound frontier memory or scanned edges. Exceeding that budget
fails; `FIND ALL SHORTEST PATHS` likewise fails only when another complete path
would exceed `max_find_paths`, so the exact boundary can succeed. Weighted
`UPTO` constrains hops; without it weighted searches are hop-unbounded, whereas
unweighted searches default to 10 steps. `UPTO` above `u32::MAX` is a parse error,
not a wrapped bound. Weighted costs must be nonnegative and
finite. Conservative reachable-weight validation may add work beyond the chosen
path, and relevant corrupt data is an error. `FIND WHERE/YIELD` is unsupported
and rejected before execution. See [FIND paths](../guide/ngql/dql.md#find-paths).

Semantic mutations currently rebuild the full current inference closure. This
is O(graph), and reads/writes of that space serialize around completion; measure
write latency and repair time on representative graphs. An interrupted closure
is marked incomplete and requires `REPAIR INFERENCE`. These guarded paths do
not establish a global-work or memory budget for every operation. Raising or disabling a guard can cause an out-of-memory process failure;
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
- The default Compose file runs one standalone server. Separate independent
  instances do not form a distributed performance topology.

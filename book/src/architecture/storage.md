# Storage engine

[한국어](../ko/architecture/storage.html)

ByoriDB uses [redb](https://www.redb.org/) as its production embedded key-value
engine. redb is a pure-Rust, copy-on-write B-tree with ACID transactions and
MVCC reads; it is not an LSM tree and ByoriDB does not expose RocksDB-style WAL,
memtable, Bloom-filter, compression, or compaction settings.

## Files and tables

A configured data path is a directory. `RedbKVStore` opens or creates:

```text
<data-path>/data.redb
```

Only the first configured data path is currently opened. The database contains
two primary tables:

| Table | Purpose |
|---|---|
| `kv` | Current graph state, schemas, indexes, users, and materialized ontology state |
| `history` | Immutable asserted vertex/edge versions and deletion tombstones |

Keeping history in a separate B-tree prevents ordinary current-view prefix
scans from sharing their tree pages with a growing history.

## Logical keyspaces

The standalone executor stores byte keys in the flat `kv` table. Important
logical namespaces include:

```text
space:<space>                              # space metadata
space:<space>:tag:<tag>                    # tag schema
space:<space>:edge:<edge-type>             # edge schema
<space>:vertex:<vid>                       # current vertex
<space>:edge:<src>:<type>:<dst>:<rank>     # current outgoing edge
<space>:in-edge:<dst>:<type>:<src>:<rank>  # reverse-edge index
<space>:tagvid:<tag>:<vid>                 # tag membership index
__user_<username>                          # durable non-root user
```

Additional namespaces hold secondary indexes, degree counters, vectors,
ontology materialization, and inference provenance. These are internal formats,
not a stable public storage API.

New vertex and edge payloads use a magic-prefixed protobuf encoding.
`VertexCodec` retains a JSON decoding fallback for legacy records. The general
row codec also contains version-aware row support, but the standalone graph DML
path stores its vertex and edge payloads through `VertexCodec`; operators should
not assume an automatic on-read schema migration that is not exercised by that
path.

## Transaction behavior

redb allows concurrent MVCC readers and serializes writers. With the default
durability, each ByoriDB write transaction commits with redb
`Durability::Immediate`.

The executor batches multi-row inserts. Its temporal `batch_apply` operation
opens both the `kv` and `history` tables in one redb write transaction, so the
current-view entity changes supplied to that call and their history versions
commit or fail together. Deletes append an empty-payload tombstone in the same
operation.

This is not a general transaction layer:

- there is no `BEGIN`/`COMMIT` query syntax;
- clauses in a compound statement execute sequentially without rollback;
- some higher-level follow-up work, such as inference or auxiliary maintenance,
  may run in additional storage operations.

## Temporal model

For asserted vertex and edge DML, ByoriDB preserves the current record and an
append-only history version:

```text
history key   = entity-key + valid-from(desc) + transaction-time(desc)
history value = valid-to + encoded entity payload
```

The current temporal surface has these boundaries:

- valid time and transaction time are both assigned from one monotonic
  epoch-millisecond value;
- multiple writes in the same wall-clock millisecond receive distinct,
  increasing transaction values;
- an insert/update writes an open `[timestamp, infinity)` version;
- a delete writes an empty tombstone;
- `FETCH PROP ON <tag> <vid> AS OF <epoch-ms>` reads a historical vertex;
- `FETCH PROP ON <edge-or-*> <src>-><dst> AS OF <epoch-ms>` reads historical
  edges, including an edge that has since been deleted;
- the one `AS OF` value is applied to both valid and transaction time.

User-provided `VALID FROM`/`VALID TO`, `BETWEEN`, temporal `GO`/`MATCH`, and
historical inferred-fact reconstruction are not implemented. The history is
for asserted vertex/edge state; ontology inference continues to use the current
view.

## Durability and cache

The server exposes two storage-specific environment variables outside the
structured `BYORIDB__...` configuration tree:

| Variable | Default | Meaning |
|---|---:|---|
| `BYORIDB_CACHE_SIZE_MB` | `256` | redb page-cache size in MiB; non-positive or invalid values fall back to the default |
| `BYORIDB_DURABILITY` | immediate | `none`, `relaxed`, or `eventual` enables relaxed bulk-load durability |

Relaxed durability skips per-commit fsync for most commits and periodically
forces a checkpoint. A crash can lose recent commits. Use it only for data that
can be reloaded, not normal serving.

On graceful shutdown the server performs an Immediate empty commit to leave
redb's allocator state clean. Give the process enough termination time to drain
queries and complete that checkpoint.

## Backup implications

The backup implementation copies both `kv` and `history` from a read
transaction into a new redb file. Copying only `data.redb` while it is changing,
or preserving only the current table, is not a supported substitute. Follow
the [Backup and restore](../operations/backup.html) procedure and test restores
regularly.

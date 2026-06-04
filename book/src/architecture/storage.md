# Storage Engine

ByoriDB uses **redb**, a pure-Rust embedded key-value store, as its underlying
storage engine. There is no C++ toolchain dependency.

## redb Architecture

redb is a single-file, copy-on-write **B-tree** store with full ACID
transactions and MVCC — not an LSM tree.

```
┌─────────────────────────────────────────────┐
│              Write Path                      │
│  begin_write → insert/remove → commit        │
│   (single writer, serialized; fsync on commit)│
└─────────────────────────────────────────────┘
                        ↓
┌─────────────────────────────────────────────┐
│            Copy-on-write B-tree              │
│  Pages are versioned; readers see a stable   │
│  MVCC snapshot while a writer commits.        │
│  Free pages are reclaimed automatically.      │
└─────────────────────────────────────────────┘
```

All rows live in a single redb table (`"kv"`) keyed by raw bytes; prefix scans
are range queries over that ordered keyspace.

## Key Encoding

### Vertex Key

```
[space_id:4][partition:4][tag_id:4][vid:8]
```

### Edge Key

```
[space_id:4][partition:4][edge_type:4][src_vid:8][rank:8][dst_vid:8]
```

### Value Encoding

```
[schema_version:4][null_bitmap:N][field_values:...]
```

The schema version enables lazy migration for online schema changes.

## Performance Tuning

redb exposes a small surface. The main knob is the page cache size:

```toml
[storage]
cache_size = "256MB"  # redb page cache; increase for read-heavy workloads
```

Durability is `Immediate` by default — every commit is fsynced and checksummed,
giving crash safety without a separate write-ahead log. (redb has no LSM
memtable/bloom-filter/compression knobs; those were RocksDB-specific.)

## Data Layout

### Vertex Storage

```
Tag Data:
┌─────────────────────────────────────────────┐
│  Key: space|part|tag|vid                    │
│  Value: version|nulls|name|age|...          │
└─────────────────────────────────────────────┘
```

### Edge Storage

Edges are stored in both directions for efficient traversal:

```
Out-Edge:
┌─────────────────────────────────────────────┐
│  Key: space|part|edge|src|rank|dst          │
│  Value: version|nulls|properties...         │
└─────────────────────────────────────────────┘

In-Edge (for reverse traversal):
┌─────────────────────────────────────────────┐
│  Key: space|part|edge|dst|rank|src          │
│  Value: (same as out-edge)                  │
└─────────────────────────────────────────────┘
```

### Index Storage

```
┌─────────────────────────────────────────────┐
│  Key: space|index_id|property_value|vid     │
│  Value: (empty or additional data)          │
└─────────────────────────────────────────────┘
```

## Schema Version Handling

For online schema changes, the storage layer handles multiple schema versions:

```
Read Path:
1. Read row from the KV store
2. Extract schema_version from row
3. If version < current:
   - Decode with old schema
   - Transform to current schema
   - Return transformed data
4. If version == current:
   - Decode directly
   - Return data
```

This lazy migration approach:
- No downtime during schema changes
- Rows updated on next write
- Gradual migration over time

## Space Reclamation

redb has no LSM compaction. As a copy-on-write B-tree it tracks free pages and
reuses them on subsequent writes automatically, so deleted keys' space is
reclaimed without a background compaction process.

## Snapshots

Point-in-time consistent snapshots:

```bash
# Create snapshot
byoridb-admin snapshot create --space my_space

# List snapshots
byoridb-admin snapshot list

# Restore from snapshot
byoridb-admin snapshot restore --id <snapshot_id>
```

Snapshots are taken by opening a read transaction (an MVCC snapshot) on the
single redb file and copying it into a self-contained backup file.

# Storage Engine

ByoriDB uses RocksDB as its underlying storage engine.

## RocksDB Architecture

```
┌─────────────────────────────────────────────┐
│              Write Path                      │
│  Write → MemTable → Immutable MemTable      │
│                          ↓                   │
│              Flush to SST Files              │
└─────────────────────────────────────────────┘
                        ↓
┌─────────────────────────────────────────────┐
│              LSM Tree                        │
│  Level 0: [SST] [SST] [SST]                 │
│  Level 1: [  SST  ] [  SST  ]               │
│  Level 2: [    SST    ] [    SST    ]       │
│              ↓ Compaction                    │
└─────────────────────────────────────────────┘
```

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

## Performance Optimizations

### Block Cache

LRU cache for frequently accessed data blocks:

```toml
[storage]
block_cache_size = "256MB"  # Increase for read-heavy workloads
```

### Bloom Filter

Probabilistic filter to reduce disk reads:

```toml
[storage]
bloom_filter_bits_per_key = 10  # ~1% false positive rate
```

### Write Buffer

In-memory buffer before flushing to disk:

```toml
[storage]
write_buffer_size = "64MB"
max_write_buffer_number = 3
```

### Compression

```toml
[storage]
compression = "lz4"  # Options: none, snappy, lz4, zstd
```

| Algorithm | Speed | Ratio | Recommendation |
|-----------|-------|-------|----------------|
| none | Fastest | 1:1 | SSD with space to spare |
| snappy | Fast | ~1.5:1 | General purpose |
| lz4 | Fast | ~2:1 | Good balance (default) |
| zstd | Slower | ~3:1 | Storage constrained |

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
1. Read row from RocksDB
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

## Compaction

RocksDB background compaction:

```toml
[storage]
max_background_compactions = 4
target_file_size_base = "64MB"
```

### Compaction Triggers

- Level 0 file count exceeds threshold
- Level size exceeds target
- Manual compaction request

### During Compaction

- Merge SST files
- Remove deleted keys
- Apply compression
- Reclaim space

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

Snapshots use RocksDB's checkpoint feature for efficient creation.

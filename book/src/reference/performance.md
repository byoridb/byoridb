# Performance

Performance characteristics and tuning guidelines.

## Benchmarks

### Test Environment

- CPU: 8-core AMD EPYC
- Memory: 32 GB
- Storage: NVMe SSD
- Dataset: 10M vertices, 100M edges

### Query Performance

| Query Type | p50 | p95 | p99 |
|------------|-----|-----|-----|
| FETCH single vertex | 0.2ms | 0.5ms | 1ms |
| FETCH batch (100) | 2ms | 5ms | 10ms |
| GO 1-hop | 1ms | 3ms | 5ms |
| GO 2-hop | 10ms | 30ms | 50ms |
| MATCH simple | 5ms | 15ms | 30ms |
| LOOKUP indexed | 1ms | 3ms | 5ms |

### Throughput

| Operation | Single Node | 3-Node Cluster |
|-----------|-------------|----------------|
| Point reads | 50K QPS | 150K QPS |
| Point writes | 20K QPS | 15K QPS |
| Mixed workload | 30K QPS | 80K QPS |

## Tuning Guidelines

### Memory Tuning

#### Block Cache

Increase for read-heavy workloads:

```toml
[storage]
block_cache_size = "4GB"  # 25% of available memory
```

#### Write Buffer

Increase for write-heavy workloads:

```toml
[storage]
write_buffer_size = "128MB"
max_write_buffer_number = 4
```

### Query Tuning

#### Use Indexes

```sql
-- Without index: full scan
LOOKUP ON person WHERE person.name == 'Alice';

-- With index: fast lookup
CREATE TAG INDEX name_idx ON person(name);
LOOKUP ON person WHERE person.name == 'Alice';
```

#### Limit Results

```sql
-- Avoid unbounded queries
MATCH (n:person) RETURN n;          -- May return millions

-- Use LIMIT
MATCH (n:person) RETURN n LIMIT 100;
```

#### Use FETCH for Known VIDs

```sql
-- If you know the vertex ID, use FETCH
FETCH PROP ON person 1;

-- Instead of
MATCH (n:person) WHERE id(n) == 1 RETURN n;
```

### Storage Tuning

#### Compression

Trade CPU for disk space:

```toml
[storage]
compression = "lz4"  # Good balance
# compression = "zstd"  # More compression, more CPU
```

#### Compaction

Tune for workload:

```toml
[storage]
# Write-heavy: more compaction threads
max_background_compactions = 4

# Read-heavy: smaller files for faster seeks
target_file_size_base = "32MB"
```

### Network Tuning

#### Connection Pooling

```toml
[client]
connection_pool_size = 10
connection_timeout_ms = 5000
```

#### Batch Operations

```sql
-- Instead of individual inserts
INSERT VERTEX person VALUES 1:('Alice', 30);
INSERT VERTEX person VALUES 2:('Bob', 25);

-- Use batch insert
INSERT VERTEX person VALUES
    1:('Alice', 30),
    2:('Bob', 25),
    3:('Carol', 28);
```

## Profiling

### Query Profiling

```sql
PROFILE {
    GO FROM 1 OVER follow YIELD $$.person.name;
}
```

Output:
```
+------------------+----------+-------+
| Operator         | Time(ms) | Rows  |
+------------------+----------+-------+
| GetNeighbors     | 2.5      | 100   |
| Project          | 0.3      | 100   |
+------------------+----------+-------+
Total: 2.8ms
```

### Explain Plan

```sql
EXPLAIN {
    MATCH (a:person)-[e:follow]->(b:person)
    WHERE a.name == 'Alice'
    RETURN b.name;
}
```

## Monitoring Performance

Key metrics to watch:

- `byoridb_query_latency_seconds` - Query latency
- `byoridb_storage_bytes` - Storage usage
- `byoridb_partition_hotspot_ratio` - Partition skew

## Common Issues

### High Latency

1. Check cache hit ratio (should be >90%)
2. Look for full scans (use EXPLAIN)
3. Verify indexes exist for query patterns

### Low Throughput

1. Check CPU utilization
2. Verify connection pool size
3. Use batch operations

### High Disk Usage

1. Enable compression
2. Check for old snapshots
3. Verify compaction is running

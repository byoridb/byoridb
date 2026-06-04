# ByoriDB

A distributed graph database written in Rust with nGQL-compatible query language.

## Overview

ByoriDB is an independent graph database implementation featuring:

- **Memory Safety**: No garbage collection, Rust's ownership model
- **Fearless Concurrency**: Safe parallel processing
- **Zero-cost Abstractions**: High-level APIs with low-level performance
- **Modern Async/Await**: Built on Tokio runtime

## Architecture

The project is organized into several crates:

| Crate | Description |
|-------|-------------|
| `byoridb-common` | Core data types (Value, Vertex, Edge, DataSet) |
| `byoridb-kvstore` | KV storage layer with RocksDB |
| `byoridb-codec` | Row encoding/decoding |
| `byoridb-storage` | Storage service for vertices and edges |
| `byoridb-meta` | Metadata management (spaces, schemas) |
| `byoridb-parser` | nGQL query language parser |
| `byoridb-executor` | Query execution engine |
| `byoridb` | Graph service and API layer |
| `byoridb-client` | Client library and CLI |

## Key Features

### Supported nGQL

**DDL (Data Definition Language)**
- `CREATE SPACE` / `DROP SPACE`
- `CREATE TAG` / `DROP TAG` / `ALTER TAG`
- `CREATE EDGE` / `DROP EDGE` / `ALTER EDGE`
- `SHOW SPACES` / `SHOW TAGS` / `SHOW EDGES`

**DML (Data Manipulation Language)**
- `INSERT VERTEX` / `UPDATE VERTEX` / `DELETE VERTEX`

**DQL (Data Query Language)**
- `FETCH PROP` - retrieve vertex properties
- `GO` - graph traversal
- `MATCH` - Cypher-style pattern matching
- `LOOKUP` - index-based queries
- `FIND PATH` - shortest path queries

### Distributed System
- **Raft Consensus**: Leader election, log replication, snapshots
- **Meta Service**: gRPC/HTTP server for schema management
- **Partitioning**: VID-based consistent hashing
- **Replica Factor**: Multi-node replication

### Performance Optimizations
- **Bloom Filter**: ~1% false positive rate
- **Block Cache**: 256MB LRU cache
- **Batch Operations**: Multi-key retrieval
- **Arena Allocation**: 16x malloc improvement
- **Predicate Pushdown**: Filter at storage layer
- **RPC Compression**: gzip/zstd support

## Quick Example

```sql
-- Create a space
CREATE SPACE my_space(vid_type=INT64);
USE my_space;

-- Define schema
CREATE TAG person(name STRING, age INT64);

-- Insert data
INSERT VERTEX person(name, age) VALUES 1:('Alice', 30);
INSERT VERTEX person(name, age) VALUES 2:('Bob', 25);

-- Query data
FETCH PROP ON person 1;
MATCH (n:person) RETURN n;
GO FROM 1 OVER * YIELD vertex;
```

## License

This project is licensed under the Apache 2.0 License.

<p align="center">
  <h1 align="center">ByoriDB</h1>
  <p align="center">A distributed graph database written in Rust with nGQL-compatible query language.</p>
</p>

<p align="center">
  <a href="#quick-start">Quick Start</a> •
  <a href="#features">Features</a> •
  <a href="#performance">Performance</a> •
  <a href="#documentation">Documentation</a> •
  <a href="#contributing">Contributing</a>
</p>

> **⚠️ Active Development** — ByoriDB is under continuous development. APIs and behaviour may change between releases. The current production deployment tracks `v0.2.x`.

---

## Why ByoriDB?

- **Safety & Speed** — Rust's memory safety with C++-level performance via zero-cost abstractions
- **Distributed by Design** — Raft consensus, consistent hashing, and horizontal scaling from day one
- **Modern Stack** — Tokio async runtime + redb (pure-Rust embedded KV), no JVM tuning or GC pauses
- **nGQL Compatible** — Familiar query language for graph operations with expanding Cypher-style support

## Quick Start

### Prerequisites

- Rust 1.90+ (pinned via `rust-toolchain.toml`)
- No C++ toolchain required — pure-Rust storage (redb)
- `protobuf-compiler` for gRPC codegen

### Build & Run

```bash
# Build
cargo build --release

# Start server
BYORIDB_ROOT_PASSWORD='<root-password>' \
  cargo run --release --bin byoridb-server

# Connect with CLI (in another terminal)
BYORIDB_USER=root BYORIDB_PASSWORD='<root-password>' \
  cargo run -p byoridb-client --bin byoridb-cli
```

### Docker (via ACR / prebuilt image)

```bash
docker run -e BYORIDB_ROOT_PASSWORD=secret \
  -p 9669:9669 -p 19669:19669 \
  byoridbacr.azurecr.io/byoridb-server:latest
```

### Your First Graph

```sql
-- Create a space
CREATE SPACE my_space(vid_type=INT64);
USE my_space;

-- Define schema
CREATE TAG person(name STRING, age INT64, city STRING);
CREATE EDGE follows(since INT64);
CREATE TAG INDEX idx_person_name ON person(name);

-- Insert data
INSERT VERTEX person(name, age, city) VALUES 1:('Alice', 30, 'Seoul');
INSERT VERTEX person(name, age, city) VALUES 2:('Bob', 25, 'London');
INSERT EDGE follows(since) VALUES 1->2:(2020);

-- Query — nGQL style
FETCH PROP ON person 1;
GO FROM 1 OVER follows YIELD $$.person.name, follows._dst;

-- Query — Cypher-style MATCH
MATCH (n:person) WHERE n.person.age > 25 RETURN n.person.name, n.person.city LIMIT 10;
MATCH (a:person)-[:follows]->(b:person) RETURN a, b LIMIT 5;

-- Statistics
SHOW STATS;
SHOW TAG INDEXES;
```

## Features

### Query Language (nGQL + Cypher extensions)

| Category | Statements |
|----------|-----------|
| **DDL** | `CREATE/DROP/ALTER SPACE/TAG/EDGE`, `CREATE/DROP TAG INDEX`, `IF NOT EXISTS / IF EXISTS` |
| **DML** | `INSERT VERTEX/EDGE`, `UPDATE VERTEX` (upsert), `DELETE VERTEX/EDGE` |
| **DQL** | `FETCH PROP ON`, `GO … OVER … YIELD`, `MATCH`, `LOOKUP`, `FIND SHORTEST PATH` |
| **MATCH** | Pattern matching, `WHERE` (AND/OR/NOT/CONTAINS/STARTS WITH/ENDS WITH/=~), `RETURN v/e` objects, `OPTIONAL MATCH`, `GROUP BY`, `ORDER BY … ASC/DESC`, `LIMIT/OFFSET` |
| **Functions** | `id(v)`, `properties(v/e)`, `tags(v)` / `labels(v)`, `COUNT/SUM/AVG/MAX/MIN`, `LOWER/UPPER/LENGTH/CONTAINS/STARTS_WITH/ENDS_WITH` |
| **Admin** | `SHOW SPACES/TAGS/EDGES/INDEXES/STATS/SESSIONS/CREATE TAG`, `EXPLAIN/PROFILE`, `REBUILD INDEX`, `BALANCE`, `GRANT/REVOKE` |

### Cypher-Style MATCH (since v0.2.x)

```sql
-- Vertex object with full tag data
MATCH (v:person) RETURN v LIMIT 1;
-- → {"v": {"vid": 1, "tags": [{"name": "person", "props": {"name": "Alice", "age": 30}}]}}

-- Properties as flat map
MATCH (v:person) RETURN id(v) AS vid, properties(v) AS props LIMIT 1;

-- Edge objects
MATCH (a)-[e:follows]->(b) RETURN e LIMIT 1;
-- → {"e": {"src": 1, "dst": 2, "type": "follows", "props": {"since": 2020}}}

-- Reverse edge patterns
MATCH (p:product)<-[:produces]-(c:company) RETURN p.product.name, c.company.name;

-- OPTIONAL MATCH (LEFT JOIN semantics)
MATCH (p:person)
OPTIONAL MATCH (p)-[:works_at]->(c:company)
RETURN p.person.name, c.company.name;

-- Regex filter
MATCH (n:person) WHERE n.person.name =~ '.*Kim' RETURN n.person.name;

-- Aggregation + GROUP BY
MATCH (n:person) RETURN n.person.city, COUNT(n) AS cnt
GROUP BY n.person.city ORDER BY cnt DESC LIMIT 5;

-- Compound statements
$f = GO FROM 1 OVER follows YIELD follows._dst AS dst;
FETCH PROP ON person $f.dst;
```

### Distributed System

- **Raft Consensus** — Leader election, log replication, snapshots
- **Consistent Hashing** — VID-based partitioning with minimal data movement (~1/N)
- **Meta Service** — Centralized schema management via gRPC/HTTP
- **Replication** — Configurable replica factor with automatic partition allocation
- **Online Schema Change** — `ALTER TAG/EDGE ADD` without downtime (lazy migration)

### Storage Engine

- redb — pure-Rust embedded KV with built-in ACID durability
- Bloom filter (10-bit/key, ~1% FPR)
- 256MB block cache (LRU)
- Batch get optimization
- Predicate pushdown at storage layer
- Configurable write-buffer limits to prevent OOM

### Security

- Role-based authentication: GOD, ADMIN, DBA, USER, GUEST
- RBAC enforcement on all statement types
- Brute-force protection (5 failed logins → 5-minute lockout)
- Session sliding-window TTL
- Random session IDs (OsRng)
- Schema validation on INSERT/UPDATE

### Operations

- gRPC and HTTP API with compression (gzip/zstd)
- CLI client (`byoridb-cli`)
- Prometheus metrics (`/metrics`)
- Structured JSON logging (compatible with ELK/Loki)
- Graceful shutdown with signal handling
- Backup/restore tool (`byoridb-backup`)
- Azure AKS deployment scripts (`deploy/azure/`)
- `SHOW SESSIONS` for active session visibility

## Architecture

```
┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│ byoridb-cli  │────▶│  byoridb     │────▶│ byoridb-     │
│   (CLI)      │     │  (Server)    │     │  executor    │
└──────────────┘     └──────────────┘     └──────┬───────┘
                                                 │
                    ┌────────────────────────────┼──────────────────┐
                    │                            │                  │
              ┌─────▼─────┐           ┌─────────▼─────────┐   ┌─────▼─────┐
              │ byoridb-  │           │   byoridb-        │   │ byoridb-  │
              │   meta    │           │   storage         │   │  parser   │
              │ (Schema)  │           │  (Raft+KV)        │   │  (nGQL)   │
              └─────┬─────┘           └─────────┬─────────┘   └───────────┘
                    │                           │
              ┌─────▼──────┐           ┌────────▼────────┐
              │ byoridb-   │           │   byoridb-      │
              │  kvstore   │           │   kvstore       │
              │ (redb)  │           │  (redb)      │
              └────────────┘           └─────────────────┘
```

| Crate | Role |
|-------|------|
| `byoridb-common` | Core data types (Value, Vertex, Edge, DataSet) |
| `byoridb-kvstore` | KV storage layer (redb, pure Rust) |
| `byoridb-codec` | Row encoding/decoding with proto/JSON dual format |
| `byoridb-storage` | Storage service, Raft consensus, indexing |
| `byoridb-meta` | Metadata management, partition allocation |
| `byoridb-parser` | nGQL query language parser (lexer + AST) |
| `byoridb-executor` | Query planning and execution engine (MATCH, GO, LOOKUP, …) |
| `byoridb-graph` | Graph service, HTTP/gRPC server, auth |
| `byoridb-client` | Client library and CLI |

## Performance

*Benchmark environment: Apple Silicon, Rust 1.90, redb*

### Query Latency

| Operation | Latency |
|-----------|---------|
| Point query (FETCH) | **143µs** |
| Batch 100 vertices | **172µs** |
| 1-hop traversal (GO) | **1.28ms** |
| 3-hop traversal | **3.41ms** |
| Index lookup (LOOKUP) | **2.98ms** |
| Full pipeline (parse→plan→execute) | **110µs** |

### Throughput (load test, single node)

| Scenario | QPS | Error rate |
|----------|-----|------------|
| 50 concurrent clients | **31 K QPS** | 0% |
| 100 concurrent clients | **12.5 K QPS** | 0% |

### BFS / Dijkstra (graph traversal bench)

| Scenario | Time | vs. baseline |
|----------|------|--------------|
| BFS chain far / 4096 nodes | 1.70 ms | −39% |
| BFS star hub 16 K neighbors | 2.12 ms | −27% |
| Dijkstra weighted / 4096 | 2.61 ms | −7% |

### Key Optimizations

| Technique | Impact |
|-----------|--------|
| Arena allocation (Bumpalo) | ~16x faster allocation |
| Bloom filter | 20–40% fewer disk reads |
| Batch get | 50–80% fewer KV round-trips |
| Predicate pushdown | 10–100x less data transfer |
| RPC compression (zstd) | 30–50% bandwidth reduction |
| `scan_stream` BoxStream | BFS hot-path −39–49% |

### Running Benchmarks

```bash
cargo bench -p byoridb-executor --bench graph_traversal
cargo bench -p byoridb-kvstore  --bench wal_overhead
```

## HTTP API (v0.2.x)

```
POST /api/v1/session          → create session (returns session_id)
DELETE /api/v1/session/:id    → sign out
POST /api/v1/query            → execute nGQL (JSON body: {session_id, query})
POST /api/v1/query/json       → same, returns raw JSON string
GET  /health                  → health check
GET  /metrics                 → Prometheus metrics
GET  /api/v1/metrics          → metrics as JSON
```

## Documentation

Full documentation is available in the [**ByoriDB Book**](book/src/SUMMARY.md).

Quick links:

- [Introduction](book/src/introduction.md) — What is ByoriDB?
- [Quick Start](book/src/getting-started/quickstart.md) — Get running in 5 minutes
- [nGQL Syntax](book/src/guide/ngql-syntax.md) — Query language reference
- [Architecture Overview](book/src/architecture/overview.md) — System design
- [Deployment](book/src/operations/deployment.md) — Production deployment
- [Project Plan](docs/PLAN.md) — Status, remaining work, decision guide

## Building from Source

```bash
# Requires Rust 1.90 (pinned in rust-toolchain.toml)
rustup update

# Debug build
cargo build

# Release build (LTO enabled)
cargo build --release

# Run all tests (serial — redb file lock contention)
cargo test --workspace -- --test-threads=1

# Run with debug logging
RUST_LOG=info cargo run --release --bin byoridb-server
```

## Known Limitations & Roadmap

| Item | Status |
|------|--------|
| Geography WKB/WKT decoding | Planned |
| MVCC / distributed 2PC transactions | Not planned (high cost vs. benefit) |
| TLS between nodes | Mitigated via network isolation; TLS termination proxy recommended |
| `RETURN *` (all variables) | Planned |
| Reverse edge index (for O(1) incoming lookup) | Planned (currently O(E)) |
| `SHOW SESSIONS` (live data) | ✅ Implemented v0.2.15 |
| Grafana dashboard templates | Planned |
| Centralized log pipeline (Fluentd/Filebeat) | Planned |

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

1. Fork the repository
2. Create your feature branch (`git checkout -b feature/amazing-feature`)
3. Enable pre-commit hooks: `git config core.hooksPath .githooks`
4. Run tests: `cargo test --workspace -- --test-threads=1`
5. Commit: `git commit -m 'feat: add amazing feature'`
6. Push and open a Pull Request

## License

This project is licensed under the [Apache 2.0 License](LICENSE).

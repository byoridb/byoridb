# Quick Start Guide

This guide will help you get ByoriDB up and running.

## Prerequisites

- **Rust**: Latest stable version (install via [rustup](https://rustup.rs/))
- **Linux/macOS**: Windows is not currently supported
- **C++ build tools**: cmake, clang, or gcc (for RocksDB compilation)

## 1. Build and Run

Clone and build the project:

```bash
git clone https://github.com/byoridb/byoridb.git
cd byoridb
cargo build --release
```

Start the standalone server (includes Meta, Storage, and Graph services):

```bash
export BYORIDB_ROOT_PASSWORD='change-me-before-production'
cargo run --release --bin byoridb-server
```

Default ports:
- **gRPC**: 9669
- **HTTP**: 19669

## 2. Connect with CLI Client

```bash
# In a new terminal
export BYORIDB_USER=root
export BYORIDB_PASSWORD='change-me-before-production'
cargo run -p byoridb-client --bin byoridb-cli
```

ByoriDB always creates the `root` user. The root password comes from
`BYORIDB_ROOT_PASSWORD`; if it is not set, the server generates a random
password and logs it once at startup.

## 3. Basic Queries (CLI)

Once connected, try running the following nGQL queries:

### Create Space

```sql
CREATE SPACE my_space(partition_num=10, replica_factor=1, vid_type=INT64);
USE my_space;
```

### Define Schema

```sql
CREATE TAG person(name STRING, age INT64);
CREATE EDGE follow(degree INT64);
```

### Insert Data

```sql
INSERT VERTEX person(name, age) VALUES 100:('Tom', 20);
INSERT VERTEX person(name, age) VALUES 101:('Jerry', 22);
INSERT EDGE follow(degree) VALUES 100->101:(95);
```

### Query Data

```sql
FETCH PROP ON person 100;
GO FROM 100 OVER follow;
LOOKUP ON person WHERE person.age > 20;
```

## 4. HTTP REST API

You can also use the HTTP API directly:

### Health Check

```bash
curl http://localhost:19669/health
# OK
```

### Create Session

```bash
curl -X POST http://localhost:19669/api/v1/session \
  -H "Content-Type: application/json" \
  -d '{"username": "root", "password": "change-me-before-production"}'
# {"session_id":1,"time_zone":"UTC"}
```

### Execute Query

```bash
# Create space
curl -X POST http://localhost:19669/api/v1/query \
  -H "Content-Type: application/json" \
  -d '{"session_id": 1, "query": "CREATE SPACE test(partition_num=10, replica_factor=1)"}'

# Use space
curl -X POST http://localhost:19669/api/v1/query \
  -H "Content-Type: application/json" \
  -d '{"session_id": 1, "query": "USE test"}'

# Create tag
curl -X POST http://localhost:19669/api/v1/query \
  -H "Content-Type: application/json" \
  -d '{"session_id": 1, "query": "CREATE TAG person(name STRING, age INT64)"}'

# Insert vertex
curl -X POST http://localhost:19669/api/v1/query \
  -H "Content-Type: application/json" \
  -d '{"session_id": 1, "query": "INSERT VERTEX person(name, age) VALUES 1:(\"Alice\", 30)"}'

# Lookup
curl -X POST http://localhost:19669/api/v1/query \
  -H "Content-Type: application/json" \
  -d '{"session_id": 1, "query": "LOOKUP ON person"}'
```

### Prometheus Metrics

```bash
curl http://localhost:19669/metrics
```

## 5. Configuration

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `BYORIDB__SERVER__GRAPH_ADDR` | `0.0.0.0:9669` | gRPC server address |
| `BYORIDB__SERVER__HTTP_ADDR` | `0.0.0.0:19669` | HTTP server address |
| `BYORIDB__STORAGE__DATA_PATHS` | `data/storage` | Data directory |

### Config File

Create `byoridb.toml`:

```toml
[server]
graph_addr = "0.0.0.0:9669"
http_addr = "0.0.0.0:19669"

[storage]
data_paths = ["data/storage"]
```

Run with config:

```bash
cargo run --release --bin byoridb-server
```

## 6. Backup and Restore

### Create Backup

```bash
cargo run --release --bin byoridb-backup -- create \
  --db data/storage \
  --backup-dir /path/to/backups \
  --label "daily"
```

### Restore Backup

```bash
cargo run --release --bin byoridb-backup -- restore \
  --backup-dir /path/to/backups \
  -i backup_20240101_120000 \
  --target /path/to/restore
```

## Next Steps

- Read [Architecture Overview](book/src/architecture/overview.md) to understand how it works
- Check [nGQL Syntax](book/src/guide/ngql-syntax.md) for complete query reference
- See [Contributing](CONTRIBUTING.md) to help improve the project

# Configuration

ByoriDB can be configured via command-line arguments or configuration files.

## Command-Line Options

### Server

The server reads `byoridb.toml` and `BYORIDB__...` environment variables.

### CLI Options

```bash
byoridb-cli [OPTIONS]

Options:
  --addr <ADDR>          Server address (default: 127.0.0.1:9669)
  --user <USER>          Username (required, env: BYORIDB_USER)
  --password <PASS>      Password (required, env: BYORIDB_PASSWORD)
  --execute <QUERY>      Execute a single query and exit
```

## Configuration File

Create a `byoridb.toml` file:

```toml
[server]
graph_addr = "0.0.0.0:9669"
http_addr = "0.0.0.0:19669"
storage_addr = "0.0.0.0:44500"

[storage]
data_paths = ["/var/lib/byoridb/storage"]
```

## Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `BYORIDB__SERVER__GRAPH_ADDR` | gRPC listen address | `0.0.0.0:9669` |
| `BYORIDB__SERVER__HTTP_ADDR` | HTTP listen address | `0.0.0.0:19669` |
| `BYORIDB__SERVER__STORAGE_ADDR` | Storage service listen address | `0.0.0.0:44500` |
| `BYORIDB__STORAGE__DATA_PATHS` | Storage data paths | `data/storage` |
| `BYORIDB_ROOT_PASSWORD` | Root user password | Generated and logged once if unset |
| `BYORIDB_USER` | CLI username | none |
| `BYORIDB_PASSWORD` | CLI password | none |

## Root User

The `root` user is always created. Set `BYORIDB_ROOT_PASSWORD` before startup
to use a known password. Without it, the server logs a generated password once.

## Directory Structure

After starting the server:

```
data/
├── meta/          # Metadata storage
├── storage/       # Graph data storage
└── wal/           # Write-ahead logs
```

## Performance Tuning

### Memory Settings

For high-performance workloads:

```toml
[storage]
block_cache_size = "1GB"
write_buffer_size = "128MB"
max_write_buffer_number = 4
```

### Compression

Enable compression to reduce disk usage:

```toml
[storage]
compression = "lz4"  # Options: none, snappy, lz4, zstd
```

## Logging

ByoriDB uses structured logging. Configure log output:

```bash
# JSON format
BYORIDB_LOG_FORMAT=json ./byoridb-server

# Enable debug logs for specific modules
RUST_LOG=byoridb_graph=debug,byoridb_storage=info ./byoridb-server
```

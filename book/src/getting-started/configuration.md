[한국어](../ko/getting-started/configuration.html)

# Configuration

The standalone server reads built-in defaults, then an optional `byoridb.toml`
in its working directory, then `BYORIDB__...` environment variables. Environment
variables use double underscores to separate configuration keys.

The server does not expose command-line flags for these settings. It accepts
only `--version` and `--help`, and rejects anything else:

```bash
$ byoridb-server --version
byoridb-server 0.3.3 (commit 9200800a1b2c, release)

$ byoridb-server --help    # lists every key and environment variable below
```

`--version` reports the commit the binary was built from, which is how a
deployed artifact is identified while there is no maintained semver release
line. A build made from a modified working tree is marked `-dirty`, and a build
made outside a git checkout reports `unknown`. Both flags exit without reading
credentials, opening storage, or binding a listener, so they are safe to run
against an installed binary at any time.

## Minimal local configuration

Use loopback listeners for a workstation-only server:

```toml
# byoridb.toml
[server]
graph_addr = "127.0.0.1:9669"
http_addr = "127.0.0.1:19669"
storage_addr = "127.0.0.1:44500"

[storage]
data_paths = ["data/storage"]
```

`storage_addr` is retained in the configuration model, but the current
standalone launcher embeds storage rather than starting a public Storage gRPC
listener.

## Server configuration keys

| Key | Environment variable | Default |
| --- | --- | --- |
| `server.graph_addr` | `BYORIDB__SERVER__GRAPH_ADDR` | `0.0.0.0:9669` |
| `server.http_addr` | `BYORIDB__SERVER__HTTP_ADDR` | `0.0.0.0:19669` |
| `server.storage_addr` | `BYORIDB__SERVER__STORAGE_ADDR` | `0.0.0.0:44500` |
| `storage.data_paths` | `BYORIDB__STORAGE__DATA_PATHS` | `data/storage` |

The parser accepts multiple data paths as a comma-separated environment value:

```bash
export BYORIDB__STORAGE__DATA_PATHS='/data/one,/data/two'
```

The current storage environment opens only the first path. Additional entries
are retained in configuration but do not provide striping or failover.

## Credentials

`BYORIDB_ROOT_PASSWORD` is separate from the double-underscore configuration
tree. The standalone binary requires a non-empty value before it starts.

```bash
export BYORIDB_ROOT_PASSWORD='value-from-your-secret-manager'
```

Root credentials cannot be rotated through nGQL. Change the managed secret and
restart the process. The server never logs the password.

The CLI uses separate variables:

| Variable | Purpose |
| --- | --- |
| `BYORIDB_USER` | Required CLI username |
| `BYORIDB_PASSWORD` | Required CLI password |

## Runtime tuning

These variables are read directly by the current process:

| Variable | Default | Meaning |
| --- | --- | --- |
| `BYORIDB_CACHE_SIZE_MB` | `256` | redb page-cache size in MiB; values must be positive |
| `BYORIDB_DURABILITY` | immediate durability | `none`, `relaxed`, or `eventual` selects relaxed durability |
| `BYORIDB_MAX_MEMORY_MB` | `1024` | Soft per-query materialized-result memory cap; `0` disables it |
| `BYORIDB_MAX_SCAN_LIMIT` | `100000` | Maximum rows from one fallback scan; `0` disables it |
| `RUST_LOG` | subscriber default | Rust tracing filter, for example `byoridb_graph=debug` |

Relaxed durability can lose recent commits after a crash. Use it only for data
that can be reloaded, not normal serving.

## Cluster settings

The configuration model also accepts:

```toml
[cluster]
node_id = 1
peers = []
advertise_addr = "127.0.0.1:9559"
bootstrap = false
meta_addr = "0.0.0.0:9559"
```

The matching variables are
`BYORIDB__CLUSTER__NODE_ID`, `BYORIDB__CLUSTER__PEERS`,
`BYORIDB__CLUSTER__ADVERTISE_ADDR`, `BYORIDB__CLUSTER__BOOTSTRAP`, and
`BYORIDB__CLUSTER__META_ADDR`. A comma-separated non-empty `peers` value enables
the Meta launcher.

Cluster startup is still incomplete: Storage/Raft peer bootstrap, deployment
wiring, and multi-node operational end-to-end coverage are not closed. Do not
treat the current cluster switches or the three services in `docker-compose.yml`
as a production distributed deployment.

## Network security

The gRPC and HTTP servers do not terminate TLS and there is no built-in
network-level login rate limiter. For any non-local deployment:

- terminate TLS at a trusted ingress or proxy;
- restrict listener access with a firewall or network policy;
- add rate limiting at the edge;
- inject `BYORIDB_ROOT_PASSWORD` from a secret manager; and
- avoid the default `0.0.0.0` listeners unless network controls are in place.

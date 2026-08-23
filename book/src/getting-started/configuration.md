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

## Sessions and login throttling

`[auth]` sets how long a bearer session lives and how failed logins are
throttled. Failed logins are limited per account, per source address, and by a
lockout; successful logins are never throttled and there is no ceiling on
concurrent logins — see [Login throttling](../reference/api.md#login-throttling)
for the wire contract. The defaults are the right choice for an exposed
listener:

```toml
[auth]
session_ttl_secs = 86400
login_window_secs = 60
max_account_failures_per_window = 20
max_source_failures_per_window = 60
max_concurrent_verifications = 4
max_failed_attempts = 5
lockout_duration_secs = 300
```

| Key | Environment variable | Default | Meaning |
| --- | --- | --- | --- |
| `auth.session_ttl_secs` | `BYORIDB__AUTH__SESSION_TTL_SECS` | `86400` | Bearer session lifetime; renewed on every use |
| `auth.login_window_secs` | `BYORIDB__AUTH__LOGIN_WINDOW_SECS` | `60` | Sliding window over which failures are counted |
| `auth.max_account_failures_per_window` | `BYORIDB__AUTH__MAX_ACCOUNT_FAILURES_PER_WINDOW` | `20` | Failures allowed per username per window |
| `auth.max_source_failures_per_window` | `BYORIDB__AUTH__MAX_SOURCE_FAILURES_PER_WINDOW` | `60` | Failures allowed per peer address per window |
| `auth.max_concurrent_verifications` | `BYORIDB__AUTH__MAX_CONCURRENT_VERIFICATIONS` | `4` | Simultaneous Argon2 verifications; excess logins queue |
| `auth.max_failed_attempts` | `BYORIDB__AUTH__MAX_FAILED_ATTEMPTS` | `5` | Consecutive failures that lock an existing account |
| `auth.lockout_duration_secs` | `BYORIDB__AUTH__LOCKOUT_DURATION_SECS` | `300` | How long that lockout lasts; `0` disables the lockout |

> **Relaxing these is only safe for a listener restricted at the network
> boundary.** They are the engine's only defence against credential guessing,
> and it has no other rate limiter to fall back on. Nothing here is adjusted
> automatically from the bind address: binding to `127.0.0.1` does not relax
> anything, because the process cannot tell a single-user desktop from a host
> reachable through a forwarded port. The decision is yours to record.

The case this exists for is a single-user deployment where a mistyped secret
locks the only account and no second administrator can recover it. Disabling
just the lockout keeps the window budgets, which still bound guessing:

```bash
export BYORIDB__AUTH__LOCKOUT_DURATION_SECS=0
```

Startup **rejects** a value that would refuse every login rather than clamping
it silently: a zero window, a zero per-account or per-source budget, zero
verification permits, a zero lockout threshold, or a zero session TTL all fail
to load with an error naming `[auth]`. Use `lockout_duration_secs = 0` to
disable the lockout; `max_failed_attempts = 0` is an error, not a way to express
that.

`max_concurrent_verifications` bounds CPU cost, not sessions. Logins beyond it
wait for a permit instead of being refused, so lowering it makes a burst slower
and never turns a correct password into a failure.

### Session lifetime

`session_ttl_secs` **slides**: every use of a session renews it for the full
TTL, so it bounds how long a session may sit *idle*, not how long it may live. A
session in continuous use never expires on its own, so shortening the TTL limits
exposure from abandoned sessions rather than from active ones — ending a session
is what `DELETE /api/v1/session` is for.

Shortening it is a meaningful control when TLS terminates at a proxy, because a
session ID is a bearer credential and neither listener offers native TLS:

```bash
export BYORIDB__AUTH__SESSION_TTL_SECS=3600
```

The accepted range is 1 second to one year. The upper bound exists because an
effectively immortal bearer credential should not be reachable by accident, and
because it catches the likeliest mistake: the unit is **seconds**, so `86400000`
is rejected rather than quietly meaning 2.7 years.

One setting governs both stores a session lives in — see
[Authentication and sessions](../reference/api.md#authentication-and-sessions).
They are reconciled on every access, so if they held separate lifetimes the
shorter one would silently become the real TTL.

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

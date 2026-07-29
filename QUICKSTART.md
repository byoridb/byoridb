# ByoriDB quick start

[한국어](QUICKSTART.ko.md)

This guide builds a local standalone server, connects with the CLI, exercises
the HTTP API, and creates a backup. Standalone single-node operation is the
primary supported path; the current cluster launcher is not production-ready.

## 1. Prerequisites

- Linux or macOS (native Windows is not currently supported)
- Rust 1.90, installed with [rustup](https://rustup.rs/) and pinned by
  `rust-toolchain.toml`
- `protobuf-compiler` (`protoc`) for gRPC code generation

The storage backend is pure Rust (redb), so a C++ build toolchain is not
required.

## 2. Build the workspace

```bash
git clone https://github.com/byoridb/byoridb.git
cd byoridb
cargo build --release
```

## 3. Start the standalone server

Set a strong root password before starting the binary:

```bash
export BYORIDB_ROOT_PASSWORD='replace-with-a-strong-local-secret'
cargo run --release --bin byoridb-server
```

`byoridb-server` fails closed when `BYORIDB_ROOT_PASSWORD` is missing, empty,
or whitespace-only. It does not print or generate a retrievable root password.
For deployment, inject this value through a secret manager instead of storing
it in a repository, image, or committed `.env` file.

Default listeners:

| Protocol | Address | Purpose |
|---|---|---|
| gRPC | `0.0.0.0:9669` | Native client and CLI |
| HTTP | `0.0.0.0:19669` | REST-style session/query API and metrics |

Confirm liveness and readiness:

```bash
curl --fail http://127.0.0.1:19669/health
# OK

curl --fail http://127.0.0.1:19669/ready
# READY
```

These listeners do not provide native TLS. Keep them on a trusted network or
place them behind TLS termination before using them outside a local environment.

## 4. Connect with the CLI

Open another terminal and provide both credentials explicitly:

```bash
export BYORIDB_USER=root
export BYORIDB_PASSWORD='replace-with-a-strong-local-secret'
cargo run -p byoridb-client --bin byoridb-cli
```

Equivalent flags are available through `--user` and `--password`, but the
password environment variable avoids placing the secret directly in shell
history and process arguments. The CLI has no default user or password.

## 5. Run basic queries

At the `byoridb>` prompt, create a space and select it:

```sql
CREATE SPACE my_space(partition_num=10, replica_factor=1, vid_type=INT64);
USE my_space;
```

Define a schema:

```sql
CREATE TAG person(name STRING, age INT64);
CREATE EDGE follows(since INT64);
```

Insert data:

```sql
INSERT VERTEX person(name, age) VALUES 100:("Tom", 20);
INSERT VERTEX person(name, age) VALUES 101:("Jerry", 22);
INSERT EDGE follows(since) VALUES 100->101:(2026);
```

Read it back:

```sql
FETCH PROP ON person 100;
GO FROM 100 OVER follows;
LOOKUP ON person WHERE person.age > 20;
MATCH (a:person)-[e:follows]->(b:person) RETURN a, e, b;
```

History is recorded for asserted vertex and edge writes. An epoch-millisecond
timestamp captured from your application can be used with the current
point-in-time read surface:

```sql
FETCH PROP ON person 100 AS OF <EPOCH_MS>;
FETCH PROP ON follows 100->101 AS OF <EPOCH_MS>;
```

Replace `<EPOCH_MS>` with the point-in-time value captured by your application.
Temporal `MATCH`, temporal `GO`,
`BETWEEN`, and user-supplied `VALID FROM/TO` are not currently supported.

## 6. Use the HTTP API

### Create a session

```bash
curl --fail-with-body -X POST http://127.0.0.1:19669/api/v1/session \
  -H 'Content-Type: application/json' \
  -d '{"username":"root","password":"replace-with-a-strong-local-secret"}'
```

The response has this shape:

```json
{"session_id":"734214891234567890","time_zone":"UTC"}
```

The session ID is emitted as a decimal JSON string because most random 63-bit
values cannot be represented exactly by a JavaScript `Number`. Preserve it as a
string and replace `<SESSION_ID>` below with the returned value. A session ID is
a bearer credential: do not publish or log it.

### Execute queries

```bash
curl --fail-with-body -X POST http://127.0.0.1:19669/api/v1/query \
  -H 'Content-Type: application/json' \
  -d '{"session_id":"<SESSION_ID>","query":"CREATE SPACE api_demo(partition_num=10, replica_factor=1)"}'

curl --fail-with-body -X POST http://127.0.0.1:19669/api/v1/query \
  -H 'Content-Type: application/json' \
  -d '{"session_id":"<SESSION_ID>","query":"USE api_demo"}'

curl --fail-with-body -X POST http://127.0.0.1:19669/api/v1/query \
  -H 'Content-Type: application/json' \
  -d '{"session_id":"<SESSION_ID>","query":"CREATE TAG person(name STRING, age INT64)"}'

curl --fail-with-body -X POST http://127.0.0.1:19669/api/v1/query \
  -H 'Content-Type: application/json' \
  -d '{"session_id":"<SESSION_ID>","query":"INSERT VERTEX person(name, age) VALUES 1:(\"Alice\", 30)"}'

curl --fail-with-body -X POST http://127.0.0.1:19669/api/v1/query \
  -H 'Content-Type: application/json' \
  -d '{"session_id":"<SESSION_ID>","query":"LOOKUP ON person"}'
```

Query strings larger than 1 MiB are rejected. `/api/v1/query/json` provides the
same query operation as a raw JSON response string.

### Inspect metrics and active queries

Prometheus metrics and the small metrics descriptor are currently unauthenticated:

```bash
curl --fail http://127.0.0.1:19669/metrics
curl --fail http://127.0.0.1:19669/api/v1/metrics
```

Active-query diagnostics require a live `GOD` or `ADMIN` session presented as a
Bearer token:

```bash
curl --fail-with-body http://127.0.0.1:19669/api/v1/diagnostics/queries \
  -H 'Authorization: Bearer <SESSION_ID>'
```

Diagnostics omit raw session IDs and redact password-bearing query text.

### Sign out

```bash
curl --fail-with-body -X DELETE \
  http://127.0.0.1:19669/api/v1/session/<SESSION_ID>
```

The sign-out route carries the bearer value in its path. Configure proxies and
access logs so this path is not recorded with the raw session ID.

## 7. Understand users and roles

`root` receives the `GOD` role. User and role administration, `SHOW USER`,
`SHOW SESSIONS`, and `BALANCE` require `GOD` or `ADMIN`. For example:

```sql
CREATE USER reader WITH PASSWORD "replace-with-a-different-secret" ROLE GUEST;
GRANT ROLE USER TO reader;
REVOKE ROLE GUEST FROM reader;
ALTER USER reader WITH PASSWORD "replace-again";
DROP USER reader;
```

Changing a user's password or role, disabling the user, or dropping the user
invalidates that user's sessions in the current server process. Live session
state is not distributed to other processes. The `root` account cannot be
created, dropped, or altered through these statements; rotate its password by
changing `BYORIDB_ROOT_PASSWORD` and restarting the server.

The built-in roles are `GOD`, `ADMIN`, `DBA`, `USER`, and `GUEST`. Their current
permissions use the wildcard space `*`; there is no space-scoped `GRANT` syntax.
Do not use these roles as a multi-tenant isolation boundary. See
[SECURITY.md](SECURITY.md) for the full deployment model.

The current introspection surface is limited: `SHOW USER` returns only the
built-in root placeholder. `SHOW SESSIONS` lists active users and selected
spaces but deliberately omits bearer session IDs. The public parser accepts
neither `SHOW USERS` nor `SHOW ROLES`.

## 8. Configure the server

Create an optional `byoridb.toml` in the working directory:

```toml
[server]
graph_addr = "127.0.0.1:9669"
http_addr = "127.0.0.1:19669"

[storage]
data_paths = ["data/storage"]
```

Configuration environment variables use a double underscore between sections
and keys:

| Variable | Default | Meaning |
|---|---|---|
| `BYORIDB_ROOT_PASSWORD` | none; required | Standalone root credential |
| `BYORIDB__SERVER__GRAPH_ADDR` | `0.0.0.0:9669` | gRPC listen address |
| `BYORIDB__SERVER__HTTP_ADDR` | `0.0.0.0:19669` | HTTP listen address |
| `BYORIDB__STORAGE__DATA_PATHS` | `data/storage` | Comma-separated data directories |
| `BYORIDB_CACHE_SIZE_MB` | `256` | redb page-cache size in MiB |
| `BYORIDB_DURABILITY` | immediate | Set to `relaxed`, `none`, or `eventual` only for reloadable bulk imports |

Relaxed durability skips per-commit fsync and can lose recent commits after a
crash. Do not use it for steady-state serving.

Cluster variables exist under `BYORIDB__CLUSTER__*`, but the end-to-end
multi-node deployment path is incomplete. Leave `cluster.peers` empty for the
supported standalone path.

## 9. Back up and restore

The backup contains both the current KV view and temporal history.

```bash
cargo run --release --bin byoridb-backup -- create \
  --db data/storage \
  --backup-dir ./backups \
  --label daily

cargo run --release --bin byoridb-backup -- list \
  --backup-dir ./backups

cargo run --release --bin byoridb-backup -- verify \
  --backup-dir ./backups \
  --backup-id <BACKUP_ID>

cargo run --release --bin byoridb-backup -- restore \
  --backup-dir ./backups \
  --backup-id <BACKUP_ID> \
  --target ./restored-data
```

Restore refuses to replace an existing target unless `--overwrite` is supplied.
Verify a restored database before replacing an active data directory.

## Next steps

- Read the [architecture overview](book/src/architecture/overview.md).
- Browse the [nGQL guide](book/src/guide/ngql-syntax.md).
- Review [security and deployment guidance](SECURITY.md).
- See [CONTRIBUTING.md](CONTRIBUTING.md) before submitting changes.

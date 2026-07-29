# API reference

[한국어](../ko/reference/api.html)

ByoriDB exposes Graph APIs over gRPC and HTTP from the same in-process
`GraphService`. They share users, roles, sessions, the selected space, active
query diagnostics, and shutdown state.

| Protocol | Default listener | Transport |
|---|---|---|
| Graph gRPC | `0.0.0.0:9669` | plaintext HTTP/2 |
| Graph HTTP | `0.0.0.0:19669` | plaintext HTTP (JSON) |

Native TLS is not implemented. Use trusted TLS termination and network access
controls outside the process.

## Authentication and sessions

The server requires a non-blank root secret at startup:

```bash
export BYORIDB_ROOT_PASSWORD='replace-with-a-managed-secret'
byoridb-server
```

The root user is created from that value and has the `GOD` role. Non-root users
are stored in redb and loaded into the authentication cache when they log in.
User/role/password changes revoke that user's existing sessions.

Session IDs are cryptographically random positive 63-bit integers. Session and
auth state is process-local: it does not survive a restart and is not shared
across replicas. The default TTL is 24 hours. Treat a session ID as a bearer
credential and never place it in logs or telemetry.

## gRPC GraphService

The source definition is `byoridb-graph/proto/graph.proto`:

```protobuf
service GraphService {
  rpc Authenticate(AuthenticateRequest) returns (AuthenticateResponse);
  rpc SignOut(SignOutRequest) returns (SignOutResponse);
  rpc Execute(ExecuteRequest) returns (ExecuteResponse);
  rpc ExecuteJson(ExecuteRequest) returns (ExecuteJsonResponse);
}

message AuthenticateRequest {
  string username = 1;
  string password = 2;
}

message AuthenticateResponse {
  int64 session_id = 1;
  int32 error_code = 2;
  string error_msg = 3;
}

message ExecuteRequest {
  int64 session_id = 1;
  string statement = 2;
}

message ExecuteResponse {
  int32 error_code = 1;
  string error_msg = 2;
  int64 latency_us = 3;
  bytes data = 4 [deprecated = true];
  DataSet result = 5;
}
```

The protobuf session ID remains an `int64`; protobuf clients preserve it
without the JavaScript JSON number precision problem.

For `Execute` and `ExecuteJson`, current application error codes are:

| Code | Meaning |
|---:|---|
| `0` | Success |
| `1` | Query, parse, authorization, or execution error |
| `2` | Session missing or expired |

`Authenticate` uses `0` for success and `1` for authentication failure.
`SignOut` currently returns `0` after attempting to remove the caller's own
session.

### Result representation

`ExecuteResponse.result` is the preferred structured result. It contains
column names and rows. Boolean, integer, float, string, and null values have
first-class protobuf variants; complex ByoriDB values currently use a
`json_value` string fallback. The deprecated `data` field is populated with a
JSON-encoded legacy result for older clients.

The server accepts gzip and zstd compressed gRPC requests and can send gzip or
zstd responses according to negotiation. Incoming decoded gRPC messages are
limited to 64 MiB.

### Rust client

```rust
use byoridb_client::Client;

let mut client = Client::connect(
    "127.0.0.1:9669".to_string(),
    "root".to_string(),
    std::env::var("BYORIDB_ROOT_PASSWORD")?,
).await?;

let text = client.execute("SHOW SPACES").await?;
let json = client.execute_json("SHOW SPACES").await?;
let response = client.execute_raw("SHOW SPACES").await?;

client.close().await?;
```

`execute_raw` is the programmatic structured-protobuf path. `execute` renders a
text table-like representation and `execute_json` returns `serde_json::Value`.

## HTTP endpoints

| Method | Path | Authentication | Purpose |
|---|---|---|---|
| `GET` | `/health` | none | Process liveness handler |
| `GET` | `/ready` | none | Query-acceptance readiness |
| `GET` | `/metrics` | none | Prometheus text metrics |
| `GET` | `/api/v1/metrics` | none | Metrics discovery JSON |
| `GET` | `/api/v1/diagnostics/queries` | GOD/ADMIN Bearer header | Active queries |
| `POST` | `/api/v1/session` | username/password body | Authenticate |
| `DELETE` | `/api/v1/session/{id}` | session ID in path | Sign out that session |
| `POST` | `/api/v1/query` | session ID in body | Execute and return JSON object |
| `POST` | `/api/v1/query/json` | session ID in body | Execute and return raw JSON text |

### Create a session

```bash
curl -sS http://127.0.0.1:19669/api/v1/session \
  -H 'Content-Type: application/json' \
  --data '{
    "username": "root",
    "password": "replace-with-a-managed-secret"
  }'
```

Response:

```json
{
  "session_id": "734817462937615829",
  "time_zone": "UTC"
}
```

HTTP serializes session IDs as **decimal strings** because most random 63-bit
values cannot be represented exactly by a JavaScript `Number`. Query requests
accept either a decimal string or a JSON integer for compatibility; clients
should send a string.

### Execute a query

```bash
curl -sS http://127.0.0.1:19669/api/v1/query \
  -H 'Content-Type: application/json' \
  --data '{
    "session_id": "734817462937615829",
    "query": "SHOW SPACES"
  }'
```

Example response shape (metadata values depend on the database):

```json
{
  "results": [
    {
      "ID": 1,
      "Name": "example",
      "Partition Num": 100,
      "Replica Factor": 1,
      "Vid Type": "INT64"
    }
  ],
  "latency_ms": 0,
  "row_count": 1,
  "column_names": ["ID", "Name", "Partition Num", "Replica Factor", "Vid Type"]
}
```

`/api/v1/query/json` accepts the same request and serializes the same fields as
raw JSON text. An HTTP query string is limited to 1 MiB; a larger request
returns HTTP 413.

Common JSON error responses from the session and `/api/v1/query` routes are:

| Route | Status | Code | Condition |
|---|---:|---|---|
| `/api/v1/query` | `400` | `QUERY_ERROR` | Parse, authorization, or execution failure |
| `/api/v1/session` | `401` | `AUTH_FAILED` | Session creation failed |
| `/api/v1/query` | `401` | `SESSION_EXPIRED` | Query session is missing or expired |
| `/api/v1/query` | `413` | `QUERY_TOO_LARGE` | Query string exceeds 1 MiB |

`/api/v1/query/json` puts `QUERY_ERROR` or `SESSION_EXPIRED` in its raw JSON
text for the corresponding failures, but its HTTP 413 response is a plain
string without a `QUERY_TOO_LARGE` code. Error text is not a stable machine
interface; use the status and a code where that route provides one.

### Sign out

```bash
curl -sS -X DELETE \
  http://127.0.0.1:19669/api/v1/session/734817462937615829
```

The current HTTP endpoint identifies the caller by the same ID in the path and
only signs out that session. Avoid retaining URLs containing live session IDs
in access logs.

### Active queries

```bash
curl -sS http://127.0.0.1:19669/api/v1/diagnostics/queries \
  -H 'Authorization: Bearer 734817462937615829'
```

The route returns HTTP 401 without a valid Bearer session and 403 unless that
session has `GOD` or `ADMIN`. Results omit session credentials and redact
password statements.

## Current limits and compatibility

- There is no application-level IP/QPS rate limiter. Enforce traffic limits at
  the proxy or network edge.
- HTTP and gRPC endpoints are versioned only by their current path/protobuf
  definitions; review release changes before upgrading clients.
- HTTP session IDs are strings on output, while gRPC uses protobuf `int64`.
- Sessions are not durable or cluster-wide.
- Health and metrics routes are unauthenticated and should be network-restricted.

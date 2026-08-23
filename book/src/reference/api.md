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

The server requires a non-empty root secret at startup:

```bash
export BYORIDB_ROOT_PASSWORD='replace-with-a-managed-secret'
byoridb-server
```

The root user is created from that value and has the `GOD` role. Non-root users
are stored in redb and hydrated into the authentication cache at server startup.
User, role, and password changes are synchronized into that cache and revoke
the affected user's existing sessions.

Session IDs are cryptographically random positive 63-bit integers. Session
state is process-local: sessions do not survive a restart and are not shared
across replicas. Treat a session ID as a bearer credential and never place it
in logs or telemetry.

The TTL defaults to 24 hours and is set by `auth.session_ttl_secs`
([Login throttling](../getting-started/configuration.md#login-throttling)). It
**slides**: every use of a session renews it for the full TTL, so the setting
bounds how long a session may sit *idle*, not how long it may live. A session
in continuous use does not expire, which is why shortening the TTL is only part
of limiting bearer exposure — `DELETE /api/v1/session` is what ends one.

A session exists in two stores that are reconciled on every access: the
authentication store, which owns identity and roles, and the graph store, which
owns the selected space. The authentication store is authoritative — if either
side is missing or expired the other is revoked, so a bearer cannot keep a
half-live session alive in one store alone. Both take their TTL from the same
setting, because the shorter of the two would otherwise be the real lifetime.

### Login throttling

There is **no ceiling on concurrent logins**. A client may open any number of
sessions at once, and a correct credential is never refused because other
logins are in flight: concurrent attempts queue on a bounded pool of four
Argon2 verifications rather than being rejected, so a burst of 32 logins costs
latency, not failures. Only the throttles below refuse an attempt, and both
are driven exclusively by *failures*:

| Control | Budget | Window | Applies to |
|---|---:|---:|---|
| Per-account failures | 20 | 60 s sliding | The presented username, whether or not it exists |
| Per-source failures | 60 | 60 s sliding | The peer IP address, across all usernames |
| Account lockout | 5 consecutive failures | 300 s | An existing account |

A successful login consumes none of these budgets and resets the account's
consecutive-failure count. This matters for clients that fan out reads: opening
many sessions is not a brute-force signal and is not treated as one.

Every refusal that did **not** evaluate a credential — a spent failure budget or
a locked account — is reported as `429 TOO_MANY_ATTEMPTS` with a `Retry-After`
header (gRPC: `error_code` 3), and is not itself counted as a failed attempt.
Retrying the same credential after that window can succeed.

Every refusal that **did** evaluate a credential is reported as
`401 AUTH_FAILED` with the body `Invalid credentials` (gRPC: `error_code` 1).
Whether the account is missing, disabled, or merely got the password wrong is
not disclosed. A locked account is the one state deliberately excluded from
this collapse — a client that cannot tell "retry later" from "wrong password"
has to stop retrying — so a caller willing to spend five wrong guesses on a
username can learn that it exists.

Every threshold above is configurable under `[auth]`; the values shown are the
defaults. See [Login throttling](../getting-started/configuration.md#login-throttling),
including how to disable the lockout for a single-user deployment and why doing
so is only safe behind a restricted listener.

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

Current application error codes are:

| RPC | Code | Meaning |
|---|---:|---|
| `Authenticate` | `0` | Success |
| `Authenticate` | `1` | Authentication failure (`Invalid credentials`) |
| `SignOut` | `0` | Success |
| `SignOut` | `1` | Other sign-out failure |
| `SignOut` | `2` | Session missing or expired |
| `Execute` / `ExecuteJson` | `0` | Success |
| `Execute` / `ExecuteJson` | `1` | Query, parse, authorization, or execution error |
| `Execute` / `ExecuteJson` | `2` | Session missing or expired |

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
| `GET` | `/api/v1/diagnostics/queries` | GOD/ADMIN session in `X-ByoriDB-Session-Id` | Active queries |
| `POST` | `/api/v1/session` | username/password body | Authenticate |
| `DELETE` | `/api/v1/session` | session in `X-ByoriDB-Session-Id` | Sign out that session |
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

### Read-only requests

Both query routes accept an optional `read_only` flag. The gRPC `ExecuteRequest`
carries the same field.

```bash
curl -sS http://127.0.0.1:19669/api/v1/query \
  -H 'Content-Type: application/json' \
  -d '{"session_id": "1234", "query": "MATCH (p:person) RETURN p LIMIT 10", "read_only": true}'
```

A read-only request is refused with `403 PERMISSION_DENIED` unless every clause
it would execute is a read. Administrative statements — `SHOW USERS`,
`SHOW ROLES`, `SHOW SESSIONS` — are refused as well, even though they otherwise
require only read permission.

The flag exists so a caller can run a statement it did not write — one supplied
by an end user or generated by a model — over a session that genuinely has write
permission, without granting that statement the session's authority. Because the
server authorizes each clause after expanding compound statements and `PROFILE`,
a caller does not have to ban semicolons, comments, or pipelines to stay safe:

- `SHOW SPACES; DELETE VERTEX 1` is refused, despite opening with a read.
- `PROFILE INSERT ...` is refused, because `PROFILE` executes its inner
  statement. Plain `EXPLAIN` does not execute it and is allowed.
- `SHOW SPACES; SHOW TAGS` is allowed, because every clause is a read.

The flag constrains **one request** and leaves no residue: the same session may
write on its next request. Omitting it is the pre-existing behavior.

It is **not** a tenant or credential boundary. Built-in roles apply to every
space, so a read-only request can still read every space its session could read.
See [Security](../operations/security.md) for what the authorization model does
and does not isolate.

Common JSON error responses from the session and `/api/v1/query` routes are:

| Route | Status | Code | Condition |
|---|---:|---|---|
| `/api/v1/query` | `400` | `QUERY_ERROR` | Parse, planning, or execution failure |
| `/api/v1/session` | `401` | `AUTH_FAILED` | The credential was evaluated and rejected |
| `/api/v1/session` | `429` | `TOO_MANY_ATTEMPTS` | Refused before the credential was evaluated; carries `Retry-After` |
| `/api/v1/query` | `401` | `SESSION_EXPIRED` | Query session is missing or expired |
| `/api/v1/query` | `403` | `PERMISSION_DENIED` | Authenticated, but the role may not run this statement |
| `/api/v1/query` | `413` | `QUERY_TOO_LARGE` | Query string exceeds 1 MiB |

The failure classes are deliberately distinct, so a client can decide what to do
from the status alone:

- **`401`** — on `/api/v1/query`, the session is gone: authenticate again, then
  re-select the space with `USE`, because a new session starts with none
  selected. On `/api/v1/session`, the credential itself was rejected: retrying
  it unchanged will fail again and will spend the account's failure budget.
- **`403`** — the session is valid and stays valid. Re-authenticating cannot help
  and only spends an attempt against the login throttle.
- **`429`** — nothing was checked. Wait out `Retry-After` and retry the same
  credential; see [Login throttling](#login-throttling).
- **`400`** — the statement itself is wrong. Retrying it unchanged will fail
  again.

Authorization failures previously reported `400` `QUERY_ERROR` with text
beginning `Authentication failed:`, which is why clients should never classify on
error text. Error text is not a stable machine interface; use the status and the
code.

`/api/v1/query/json` returns the same statuses and codes in its raw JSON text,
except that its HTTP 413 response is a plain string without a `QUERY_TOO_LARGE`
code.

### Sign out

```bash
curl -sS -X DELETE \
  http://127.0.0.1:19669/api/v1/session \
  -H 'X-ByoriDB-Session-Id: 734817462937615829'
```

The endpoint validates and signs out the session identified by the header.
A missing or malformed header returns HTTP 401 with `AUTH_REQUIRED`; a parsed
session that is expired or unknown returns HTTP 401 with `SESSION_EXPIRED`.

### Active queries

```bash
curl -sS http://127.0.0.1:19669/api/v1/diagnostics/queries \
  -H 'X-ByoriDB-Session-Id: 734817462937615829'
```

The route returns HTTP 401 without a valid session header and 403 unless that
session has `GOD` or `ADMIN`. Results contain only `id`, `query_type`,
`query_length_bytes`, `space`, and `started_at_ms`; raw query text and session
IDs are omitted.

## Current limits and compatibility

- There is no application-level IP/QPS rate limiter. Enforce traffic limits at
  the proxy or network edge.
- HTTP and gRPC endpoints are versioned only by their current path/protobuf
  definitions; review release changes before upgrading clients.
- HTTP session IDs are strings on output, while gRPC uses protobuf `int64`.
- Sessions are not durable or cluster-wide.
- Health and metrics routes are unauthenticated and should be network-restricted.

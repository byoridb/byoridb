# API 레퍼런스

[English](../../reference/api.html)

ByoriDB는 동일한 in-process `GraphService`에서 gRPC와 HTTP Graph API를 제공합니다.
두 protocol은 user, role, session, 선택한 space, active query diagnostics, shutdown 상태를
공유합니다.

| Protocol | 기본 listener | Transport |
|---|---|---|
| Graph gRPC | `0.0.0.0:9669` | plaintext HTTP/2 |
| Graph HTTP | `0.0.0.0:19669` | plaintext HTTP (JSON) |

native TLS는 구현되어 있지 않습니다. process 외부에서 신뢰할 수 있는 TLS termination과
network access control을 사용하세요.

## 인증과 session

server 시작 시 비어 있지 않은 root secret이 필요합니다.

```bash
export BYORIDB_ROOT_PASSWORD='replace-with-a-managed-secret'
byoridb-server
```

root user는 이 값으로 만들어지며 `GOD` role을 가집니다. non-root user는 redb에 저장되고
로그인할 때 authentication cache로 읽힙니다. user/role/password 변경은 해당 user의 기존
session을 폐기합니다.

session ID는 cryptographically random한 양의 63-bit integer입니다. session/auth 상태는
process-local이며 restart 후 사라지고 replica 사이에 공유되지 않습니다. 기본 TTL은
24시간입니다. session ID를 bearer credential로 취급하고 log/telemetry에 넣지 마세요.

## gRPC GraphService

source definition은 `byoridb-graph/proto/graph.proto`입니다.

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

protobuf session ID는 `int64`이며 protobuf client는 JavaScript JSON number precision
문제 없이 값을 보존합니다.

`Execute`와 `ExecuteJson`의 현재 application error code는 다음과 같습니다.

| Code | 의미 |
|---:|---|
| `0` | 성공 |
| `1` | query, parse, authorization, execution error |
| `2` | session 없음 또는 만료 |

`Authenticate`는 성공 시 `0`, 인증 실패 시 `1`을 사용합니다. `SignOut`은 caller 자신의
session을 제거하려고 시도한 뒤 현재 `0`을 반환합니다.

### Result 표현

`ExecuteResponse.result`가 권장 structured result입니다. column name과 row를 포함합니다.
boolean, integer, float, string, null은 first-class protobuf variant이며 complex ByoriDB
value는 현재 `json_value` string fallback을 사용합니다. deprecated `data` field에는 과거
client를 위해 JSON-encoded legacy result도 채웁니다.

server는 gzip/zstd compressed gRPC request를 받고 협상에 따라 gzip 또는 zstd response를
보낼 수 있습니다. decode된 incoming gRPC message limit은 64 MiB입니다.

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

`execute_raw`가 programmatic structured-protobuf 경로입니다. `execute`는 text table 형태로
render하고 `execute_json`은 `serde_json::Value`를 반환합니다.

## HTTP endpoint

| Method | Path | 인증 | 용도 |
|---|---|---|---|
| `GET` | `/health` | 없음 | process liveness handler |
| `GET` | `/ready` | 없음 | query-acceptance readiness |
| `GET` | `/metrics` | 없음 | Prometheus text metrics |
| `GET` | `/api/v1/metrics` | 없음 | metrics discovery JSON |
| `GET` | `/api/v1/diagnostics/queries` | GOD/ADMIN Bearer header | active query |
| `POST` | `/api/v1/session` | username/password body | 인증 |
| `DELETE` | `/api/v1/session/{id}` | path의 session ID | 해당 session sign out |
| `POST` | `/api/v1/query` | body의 session ID | 실행 후 JSON object 반환 |
| `POST` | `/api/v1/query/json` | body의 session ID | 실행 후 raw JSON text 반환 |

### Session 생성

```bash
curl -sS http://127.0.0.1:19669/api/v1/session \
  -H 'Content-Type: application/json' \
  --data '{
    "username": "root",
    "password": "replace-with-a-managed-secret"
  }'
```

응답:

```json
{
  "session_id": "734817462937615829",
  "time_zone": "UTC"
}
```

대부분의 random 63-bit 값은 JavaScript `Number`로 정확히 표현할 수 없으므로 HTTP는
session ID를 **decimal string**으로 serialize합니다. query request는 compatibility를
위해 decimal string과 JSON integer를 모두 받지만 client는 string을 보내야 합니다.

### Query 실행

```bash
curl -sS http://127.0.0.1:19669/api/v1/query \
  -H 'Content-Type: application/json' \
  --data '{
    "session_id": "734817462937615829",
    "query": "SHOW SPACES"
  }'
```

응답 형태 예시입니다. metadata 값은 database에 따라 달라집니다.

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

`/api/v1/query/json`은 동일한 request를 받고 같은 field를 raw JSON text로 serialize합니다.
HTTP query string limit은 1 MiB이며 더 크면 HTTP 413을 반환합니다.

session과 `/api/v1/query` route의 일반적인 JSON error response는 다음과 같습니다.

| Route | Status | Code | 조건 |
|---|---:|---|---|
| `/api/v1/query` | `400` | `QUERY_ERROR` | parse, authorization, execution failure |
| `/api/v1/session` | `401` | `AUTH_FAILED` | session 생성 실패 |
| `/api/v1/query` | `401` | `SESSION_EXPIRED` | query session 없음 또는 만료 |
| `/api/v1/query` | `413` | `QUERY_TOO_LARGE` | query string이 1 MiB 초과 |

`/api/v1/query/json`은 대응 failure에서 raw JSON text에 `QUERY_ERROR` 또는
`SESSION_EXPIRED`를 넣지만 HTTP 413 response는 `QUERY_TOO_LARGE` code가 없는 plain
string입니다. error text는 안정적인 machine interface가 아닙니다. route가 제공하는
status와 code를 사용하세요.

### Sign out

```bash
curl -sS -X DELETE \
  http://127.0.0.1:19669/api/v1/session/734817462937615829
```

현재 HTTP endpoint는 path의 같은 ID로 caller를 식별하고 해당 session만 sign out합니다.
live session ID가 든 URL을 access log에 보존하지 마세요.

### Active query

```bash
curl -sS http://127.0.0.1:19669/api/v1/diagnostics/queries \
  -H 'Authorization: Bearer 734817462937615829'
```

유효한 Bearer session이 없으면 HTTP 401, 해당 session에 `GOD` 또는 `ADMIN`이 없으면
403을 반환합니다. 결과는 session credential을 제외하고 password statement를
redaction합니다.

## 현재 limit과 compatibility

- application-level IP/QPS rate limiter가 없습니다. proxy/network edge에서 traffic
  limit을 적용하세요.
- HTTP/gRPC endpoint는 현재 path/protobuf definition으로만 versioning됩니다. client
  upgrade 전에 release change를 검토하세요.
- HTTP session ID는 output에서 string이고 gRPC는 protobuf `int64`입니다.
- session은 durable하거나 cluster-wide하지 않습니다.
- health/metrics route는 인증이 없으므로 network restriction이 필요합니다.

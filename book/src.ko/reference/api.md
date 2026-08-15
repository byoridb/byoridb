# API 레퍼런스

[English](../../reference/api.html)

ByoriDB는 gRPC와 HTTP API를 제공합니다.

## 연결

### 기본 포트

| 서비스 | gRPC 포트 | HTTP 포트 |
|---------|-----------|-----------|
| Graph | 9669 | 19669 |
| Meta | 9559 | - |
| Storage | standalone에서는 별도 listener 없음 | - |

Meta listener는 `cluster.peers`를 설정한 launcher 경로에서만 열립니다. standalone의
StorageServer는 같은 프로세스에서 redb를 열어 Graph service와 공유하며 9779 gRPC
listener를 시작하지 않습니다.

### 인증

ByoriDB는 시작 시 `root` 슈퍼유저를 생성합니다. network server는 시작 전에
`BYORIDB_ROOT_PASSWORD`가 반드시 설정돼 있어야 하며, 없거나 빈 값이면 fail-fast합니다.
공백으로만 구성된 값에 대한 별도 강도 검사는 현재 수행하지 않습니다. Credential은
로그에 출력되지 않습니다.

영속 사용자는 redb에 저장되고 서버 시작 시 authentication cache에 적재됩니다.
사용자, role, 비밀번호 변경은 cache에 동기화되며 해당 사용자의 local session을
폐기합니다. Session은 기본 24시간 수명의 암호학적으로 무작위인 양의 63-bit bearer
credential이며, 프로세스를 재시작하거나 다른 replica로 이동해 유지되지 않습니다.

## gRPC API

### 서비스 정의

```protobuf
service GraphService {
    rpc Authenticate(AuthenticateRequest) returns (AuthenticateResponse);
    rpc SignOut(SignOutRequest) returns (SignOutResponse);
    rpc Execute(ExecuteRequest) returns (ExecuteResponse);
    rpc ExecuteJson(ExecuteRequest) returns (ExecuteJsonResponse);
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

### 클라이언트 연결

```rust
use byoridb_client::Client;

let mut client = Client::connect(
    "localhost:9669".to_string(),
    "root".to_string(),
    std::env::var("BYORIDB_ROOT_PASSWORD")?,
).await?;

let result = client.execute("SHOW SPACES").await?;
```

### 세션 관리

세션은 인증 중에 생성됩니다. Rust 클라이언트는 `Client::connect` 이후 세션 ID를
내부적으로 관리합니다.

## HTTP API

### 엔드포인트

| 엔드포인트 | 메서드 | 설명 |
|----------|--------|-------------|
| `/health` | GET | 헬스 체크 |
| `/ready` | GET | 새 query 수락 readiness |
| `/metrics` | GET | Prometheus 지표 |
| `/api/v1/metrics` | GET | `/metrics`를 안내하는 JSON 응답 |
| `/api/v1/session` | POST | 인증된 세션 생성 |
| `/api/v1/session` | DELETE | `X-ByoriDB-Session-Id`의 현재 세션 종료 |
| `/api/v1/query` | POST | 쿼리 실행 |
| `/api/v1/query/json` | POST | 쿼리를 실행하고 JSON 반환 |
| `/api/v1/diagnostics/queries` | GET | `X-ByoriDB-Session-Id`를 사용하는 GOD/ADMIN용 실행 중 쿼리 metadata |

### 세션 생성

```bash
curl -X POST http://localhost:19669/api/v1/session \
  -H "Content-Type: application/json" \
  -d '{
    "username": "root",
    "password": "change-me-before-production"
  }'
```

HTTP 응답의 `session_id`는 JavaScript 정밀도 손실을 피하기 위해 decimal string으로
직렬화됩니다. 쿼리 body에도 문자열 그대로 전달하세요.

### 세션 종료

세션 ID는 bearer credential이므로 URL에 넣지 않고 header로 전달합니다.

```bash
curl -X DELETE http://localhost:19669/api/v1/session \
  -H "X-ByoriDB-Session-Id: <SESSION_ID>"
# {"deleted":true}
```

Header가 없거나 형식이 잘못됐으면 HTTP 401과 `AUTH_REQUIRED`를 반환합니다.
Parsing된 session이 만료됐거나 미등록 상태면 HTTP 401과 `SESSION_EXPIRED`를
반환합니다.

### 실행 중 쿼리 진단

```bash
curl http://localhost:19669/api/v1/diagnostics/queries \
  -H "X-ByoriDB-Session-Id: <GOD_OR_ADMIN_SESSION_ID>"
```

응답은 `id`, `query_type`, `query_length_bytes`, `space`, `started_at_ms`만 포함합니다.
raw query와 bearer session ID는 반환하지 않습니다.

### 쿼리 실행

```bash
curl -X POST http://localhost:19669/api/v1/query \
  -H "Content-Type: application/json" \
  -d '{
    "session_id": "<SESSION_ID>",
    "query": "SHOW SPACES"
  }'
```

응답:

```json
{
  "results": [{"Name": "my_space"}, {"Name": "test_space"}],
  "latency_ms": 1,
  "row_count": 2,
  "column_names": ["Name"]
}
```

### 헬스 체크

```bash
curl http://localhost:19669/health

# Response
OK
```

### 지표(Metrics)

```bash
curl http://localhost:19669/metrics

# Response (Prometheus format)
# HELP byoridb_query_total Total number of queries executed
# TYPE byoridb_query_total counter
byoridb_query_total{space="default",type="show"} 12
```

## 에러 코드

현재 Graph gRPC 응답의 `error_code`는 작은 transport-level 집합입니다.

| RPC | 코드 | 설명 |
|-----|------|------|
| Authenticate | 0 | 성공 |
| Authenticate | 1 | 인증 실패 (`Invalid credentials`) |
| SignOut | 0 | 성공 |
| SignOut | 1 | 그 밖의 sign-out 실패 |
| SignOut | 2 | session이 없거나 만료됨 |
| Execute / ExecuteJson | 0 | 성공 |
| Execute / ExecuteJson | 1 | parse/planning/execution 등 query 오류 |
| Execute / ExecuteJson | 2 | session이 없거나 만료됨 |

HTTP API는 상태 코드와 문자열 `code`를 함께 반환합니다. 인증/인가 경로의 주요 값은
`AUTH_FAILED`, `AUTH_REQUIRED`, `FORBIDDEN`, `SESSION_EXPIRED`이고, query 길이 제한은
`/api/v1/query`에서 query 문자열이 1 MiB를 넘으면 HTTP 413과 `QUERY_TOO_LARGE`입니다.
query 실패는 다음 세 부류로 갈라지며, client는 status만으로 다음 행동을 결정할 수
있습니다:

| Status | Code | 조건 | client가 할 일 |
|---:|---|---|---|
| `401` | `SESSION_EXPIRED` | session이 없거나 만료됨 | 재인증 후 `USE`로 space 재선택 |
| `403` | `PERMISSION_DENIED` | 인증은 됐지만 해당 role로는 실행 불가 | 재인증해도 해결되지 않음 |
| `400` | `QUERY_ERROR` | parse·planning·execution 실패 | 같은 문장을 재시도해도 실패 |

`401`은 session이 사라진 경우입니다. 새 session에는 space가 선택되어 있지 않으므로
재인증 후 `USE`를 다시 실행해야 합니다. `403`에서는 session이 그대로 유효하며,
재인증은 도움이 되지 않고 login throttle 시도만 소모합니다.

인가 실패는 이전에 `Authentication failed:`로 시작하는 본문과 함께 `400`
`QUERY_ERROR`로 반환되었습니다. client가 error text로 분류해서는 안 되는 이유입니다.
error text는 안정적인 machine interface가 아니므로 status와 code를 사용하세요.

`/api/v1/query/json`도 같은 status와 code를 반환하지만, 길이 제한 응답만은 현재
structured `code` 없이 plain string입니다.
NebulaGraph의 음수 error code 전체를 구현한 것으로 가정하면 안 됩니다.

## 데이터 타입

### Protocol Buffer 타입

```protobuf
message Value {
    oneof value {
        NullValue null_value = 1;
        bool bool_value = 2;
        int64 int_value = 3;
        double float_value = 4;
        string string_value = 5;
        string json_value = 6;
    }
}
```

gRPC는 null/bool/int/float/string만 oneof에 직접 담고 Vertex, Edge, Path, collection,
date/time 계열 같은 복합 값은 현재 `json_value` 문자열로 fallback합니다.

### JSON 타입

| nGQL 타입 | JSON 타입 |
|-----------|-----------|
| BOOL | boolean |
| INT8/16/32/64 | number |
| FLOAT/DOUBLE | number |
| STRING | string |
| LIST | array |
| MAP | object |
| VERTEX / EDGE / PATH | object |
| 그 밖의 복합/시간 타입 | 현재 Debug 문자열 fallback(안정된 wire schema 아님) |

## 속도 제한(Rate Limiting)

내장 IP별 연결 제한이나 초당 query rate limiter는 아직 없습니다. 필요하면 ingress/API
gateway에서 적용하세요. HTTP query body의 `query` 문자열에는 코드에 고정된 1 MiB 제한이
있지만 이를 변경하는 `[limits]` TOML 설정은 구현돼 있지 않습니다. executor config에
timeout 필드는 존재하나 현재 query 실행을 취소하는 timeout enforcement로 연결되지
않았으므로 운영 timeout/SLO로 간주하면 안 됩니다.

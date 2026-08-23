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
폐기합니다. Session은 암호학적으로 무작위인 양의 63-bit bearer credential이며,
프로세스를 재시작하거나 다른 replica로 이동해 유지되지 않습니다.

TTL은 기본 24시간이고 `auth.session_ttl_secs`로 설정합니다([로그인 throttling](../getting-started/configuration.md#로그인-throttling)).
이 TTL은 **sliding**입니다 — session을 사용할 때마다 전체 TTL만큼 갱신되므로, 이
설정은 session이 얼마나 오래 *유휴* 상태로 있을 수 있는지를 제한하며 총 수명을
제한하지 않습니다. 계속 사용되는 session은 만료되지 않습니다. TTL을 줄이는 것만으로
bearer 노출을 제한할 수 없고, session을 실제로 끝내는 것은
`DELETE /api/v1/session`입니다.

Session은 접근마다 재조정되는 두 저장소에 존재합니다 — 신원과 role을 소유하는
authentication 저장소, 그리고 선택된 space를 소유하는 graph 저장소입니다.
authentication 저장소가 authoritative이며, 한쪽이 없거나 만료되면 다른 쪽도 폐기되므로
bearer가 한 저장소에만 반쯤 살아 있는 session을 유지할 수 없습니다. 두 저장소는 같은
설정에서 TTL을 가져옵니다 — 그러지 않으면 둘 중 짧은 쪽이 실제 수명이 됩니다.

### Login throttling

**동시 로그인 수에는 상한이 없습니다.** client는 session을 몇 개든 동시에 열 수 있고,
다른 로그인이 진행 중이라는 이유로 정상 credential이 거부되는 일은 없습니다. 동시
시도는 거부되지 않고 4개짜리 Argon2 검증 pool에서 대기하므로, 32개를 한꺼번에 보내면
실패가 아니라 지연만 발생합니다. 아래 throttle만이 시도를 거부하며, 셋 다 **실패**만
집계합니다:

| 제어 | Budget | Window | 적용 대상 |
|---|---:|---:|---|
| 계정별 실패 | 20 | 60초 sliding | 제시된 username (존재하지 않아도 적용) |
| 출처별 실패 | 60 | 60초 sliding | peer IP, username 무관 |
| 계정 lockout | 연속 5회 실패 | 300초 | 존재하는 계정 |

성공한 로그인은 이 budget을 전혀 소모하지 않고 계정의 연속 실패 횟수를 초기화합니다.
읽기를 fan-out하는 client에 중요한 지점입니다 — session을 많이 여는 것은 brute-force
신호가 아니며 그렇게 취급되지도 않습니다.

credential을 **평가하지 않고** 거부한 경우 — 실패 budget 소진 또는 계정 lockout — 는
`Retry-After` header와 함께 `429 TOO_MANY_ATTEMPTS`로 반환되고(gRPC는 `error_code` 3),
그 거부 자체는 실패로 집계되지 않습니다. 해당 window가 지난 뒤 같은 credential로
재시도하면 성공할 수 있습니다.

credential을 **평가한** 거부는 `401 AUTH_FAILED`와 본문 `Invalid credentials`로
반환됩니다(gRPC는 `error_code` 1). 계정이 없는지, 비활성인지, 비밀번호만 틀렸는지는
노출하지 않습니다. 계정 lockout은 이 통합에서 의도적으로 제외한 유일한 상태입니다 —
"나중에 재시도"와 "비밀번호가 틀림"을 구분할 수 없는 client는 재시도를 멈춰야 하기
때문입니다. 그 대가로, 어떤 username에 틀린 추측을 5회 쓸 의지가 있는 호출자는 그
계정의 존재를 알아낼 수 있습니다.

위 임계값은 모두 `[auth]` 아래에서 설정할 수 있고, 표시된 값이 기본값입니다.
단일 사용자 배포에서 lockout을 비활성화하는 방법과 그것이 제한된 listener 뒤에서만
안전한 이유는 [로그인 throttling](../getting-started/configuration.md#로그인-throttling)을
참고하세요.

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

### 읽기 전용 요청

두 query route 모두 선택적 `read_only` flag를 받습니다. gRPC `ExecuteRequest`에도
같은 field가 있습니다.

```bash
curl -sS http://127.0.0.1:19669/api/v1/query \
  -H 'Content-Type: application/json' \
  -d '{"session_id": "1234", "query": "MATCH (p:person) RETURN p LIMIT 10", "read_only": true}'
```

읽기 전용 요청은 실행될 모든 절이 읽기가 아니면 `403 PERMISSION_DENIED`로 거부됩니다.
`SHOW USERS`, `SHOW ROLES`, `SHOW SESSIONS`는 읽기 권한만 필요하지만 관리 문장이므로
함께 거부됩니다.

이 flag는 **직접 작성하지 않은 문장** — 최종 사용자가 넣거나 모델이 생성한 문장 — 을
쓰기 권한이 있는 session으로 실행하면서도 그 문장에 session의 권한을 주지 않기 위한
것입니다. server가 compound와 `PROFILE`을 펼친 뒤 절 단위로 인가하므로, 호출자가
안전을 위해 세미콜론·주석·pipeline을 금지할 필요가 없습니다:

- `SHOW SPACES; DELETE VERTEX 1`은 읽기로 시작하더라도 거부됩니다.
- `PROFILE INSERT ...`는 `PROFILE`이 내부 문장을 실행하므로 거부됩니다. 일반
  `EXPLAIN`은 실행하지 않으므로 허용됩니다.
- `SHOW SPACES; SHOW TAGS`는 모든 절이 읽기이므로 허용됩니다.

flag는 **요청 하나만** 제약하고 흔적을 남기지 않습니다. 같은 session은 다음 요청에서
쓸 수 있습니다. 생략하면 기존 동작과 동일합니다.

**tenant나 credential 경계가 아닙니다.** built-in role은 모든 space에 적용되므로,
읽기 전용 요청도 해당 session이 읽을 수 있는 모든 space를 읽습니다. 권한 모델이 무엇을
격리하고 무엇을 격리하지 않는지는 [보안](../operations/security.md)을 참고하세요.

HTTP API는 상태 코드와 문자열 `code`를 함께 반환합니다. 인증/인가 경로의 주요 값은
`AUTH_FAILED`, `AUTH_REQUIRED`, `FORBIDDEN`, `SESSION_EXPIRED`, `TOO_MANY_ATTEMPTS`이고,
query 길이 제한은 `/api/v1/query`에서 query 문자열이 1 MiB를 넘으면 HTTP 413과
`QUERY_TOO_LARGE`입니다. 실패는 다음 부류로 갈라지며, client는 status만으로 다음
행동을 결정할 수 있습니다:

| Status | Code | 조건 | client가 할 일 |
|---:|---|---|---|
| `401` | `SESSION_EXPIRED` | session이 없거나 만료됨 | 재인증 후 `USE`로 space 재선택 |
| `401` | `AUTH_FAILED` | credential이 평가되어 거부됨 | 같은 credential을 재시도해도 실패 |
| `403` | `PERMISSION_DENIED` | 인증은 됐지만 해당 role로는 실행 불가 | 재인증해도 해결되지 않음 |
| `429` | `TOO_MANY_ATTEMPTS` | credential을 평가하기 전에 거부됨 | `Retry-After`만큼 기다린 뒤 같은 credential로 재시도 |
| `400` | `QUERY_ERROR` | parse·planning·execution 실패 | 같은 문장을 재시도해도 실패 |

`401`은 `/api/v1/query`에서는 session이 사라진 경우입니다. 새 session에는 space가
선택되어 있지 않으므로 재인증 후 `USE`를 다시 실행해야 합니다. `/api/v1/session`에서는
credential 자체가 거부된 것이므로, 그대로 재시도하면 계정의 실패 budget만 소모합니다.
`403`에서는 session이 그대로 유효하며, 재인증은 도움이 되지 않고 login throttle 시도만
소모합니다. `429`는 아무것도 검사되지 않은 경우입니다([login throttling](#login-throttling)).

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

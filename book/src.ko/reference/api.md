# API 레퍼런스

[English](../../reference/api.html)

ByoriDB는 동일한 프로세스 안의 `GraphService`에서 gRPC와 HTTP Graph API를
제공합니다. 두 API는 사용자, 역할, session, 선택한 space, active-query diagnostics,
종료 상태를 공유합니다.

이 페이지는 구현된 query API를 설명합니다.
[Agent-first 설계](../architecture/agent-first.html)는 계획 중인 스키마 discovery,
타입 있는 연산, 제한된 근거 응답, 변경 결과 계약을 정의합니다. 그 계약이 이 페이지의
현재 API에 새 필드나 endpoint가 추가됐다는 뜻은 아닙니다.

| 프로토콜 | 기본 listener | 전송 방식 |
|---|---|---|
| Graph gRPC | `0.0.0.0:9669` | plaintext HTTP/2 |
| Graph HTTP | `0.0.0.0:19669` | plaintext HTTP (JSON) |

Native TLS는 구현되지 않았습니다. 프로세스 외부에서 신뢰할 수 있는 TLS termination과
network access control을 적용하세요.

<a id="authentication-and-sessions"></a>

## 인증과 세션

서버는 시작 시 비어 있지 않은 root secret을 요구합니다.

```bash
export BYORIDB_ROOT_PASSWORD='replace-with-a-managed-secret'
byoridb-server
```

Root 사용자는 이 값으로 생성되며 `GOD` role을 가집니다. Non-root 사용자는 redb에
저장되고 서버 시작 시 authentication cache에 로드됩니다. 사용자, 역할, 비밀번호
변경은 cache에 동기화되며 해당 사용자의 기존 session을 폐기합니다.

Session ID는 암호학적으로 무작위인 양의 63-bit 정수입니다. Session 상태는
process-local이므로 재시작 후 유지되지 않으며 replica 간에 공유되지 않습니다.
Session ID를 bearer credential로 취급하고 로그나 telemetry에 남기지 마세요.

TTL은 기본 24시간이며 `auth.session_ttl_secs`로 설정합니다
([세션과 로그인 제한](../getting-started/configuration.md#sessions-and-login-throttling)).
TTL은 **sliding** 방식으로 session을 사용할 때마다 전체 수명만큼 갱신됩니다. 따라서
session이 *미사용 상태*로 머무는 시간을 제한하며 전체 생존 시간을 제한하지 않습니다.
계속 사용 중인 session은 만료되지 않으므로 TTL을 줄이는 것만으로 bearer 노출을 모두
제한할 수 없습니다. Session을 종료하려면 `DELETE /api/v1/session`을 사용합니다.

Session은 매 접근마다 조정되는 두 저장소에 존재합니다. Authentication store는
identity와 role을, graph store는 선택된 space를 보관합니다. Authentication store가
권한의 기준이며, 어느 한쪽이 없거나 만료되면 나머지도 폐기됩니다. 따라서 bearer가
한쪽에만 남은 session을 계속 유지할 수 없습니다. 두 저장소는 같은 설정의 TTL을
사용합니다. 값이 다르면 더 짧은 쪽이 실제 수명이 되기 때문입니다.

<a id="login-throttling"></a>

### 로그인 제한

**동시 로그인 개수에는 상한이 없습니다.** Client는 여러 session을 동시에 열 수
있으며 다른 로그인이 실행 중이라는 이유로 올바른 credential을 거부하지 않습니다.
동시 요청은 기본 4개짜리 Argon2 검증 pool에서 대기하므로 한 번에 32개를 보내면 실패
대신 지연이 늘어납니다. 요청을 거부하는 다음 제한은 모두 *실패*를 기준으로 합니다.

| 제한 | 한도 | 기간 | 적용 대상 |
|---|---:|---:|---|
| 계정별 실패 | 20 | sliding 60초 | 존재 여부와 관계없이 입력한 username |
| Source별 실패 | 60 | sliding 60초 | 모든 username에 걸친 peer IP address |
| 계정 잠금 | 연속 5회 실패 | 300초 | 존재하는 계정 |

성공한 로그인은 이 한도를 소비하지 않으며 해당 계정의 연속 실패 횟수를 초기화합니다.
여러 읽기를 병렬로 수행하는 client가 session을 많이 여는 것 자체를 비밀번호 추측
공격으로 취급하지 않습니다.

실패 한도를 소진했거나 계정이 잠겨 credential을 **검증하지 않은** 거부는
`Retry-After` header를 포함한 `429 TOO_MANY_ATTEMPTS`로 반환합니다
(gRPC: `error_code` 3). 이 거부 자체는 실패 횟수에 포함되지 않습니다. 해당 기간이
지난 뒤 같은 credential로 다시 시도하면 성공할 수 있습니다.

Credential을 **검증한 뒤** 거부한 경우 `Invalid credentials` 본문과
`401 AUTH_FAILED`를 반환합니다(gRPC: `error_code` 1). 계정이 존재하지 않거나
비활성 상태인지, 비밀번호가 틀렸는지는 구분해 노출하지 않습니다. 잠긴 계정은 이
통합 응답에서 의도적으로 제외합니다. Client가 "나중에 재시도"와 "잘못된 비밀번호"를
구분할 수 있어야 하기 때문입니다. 따라서 특정 username에 잘못된 비밀번호를 5번
제시할 의사가 있는 caller는 그 계정의 존재 여부를 알아낼 수 있습니다.

위 임계값은 모두 `[auth]`에서 설정할 수 있으며 표는 기본값입니다. 단일 사용자
배포에서 lockout을 끄는 방법과 listener 접근을 제한한 환경에서만 완화해야 하는 이유는
[세션과 로그인 제한](../getting-started/configuration.md#sessions-and-login-throttling)을
참고하세요.

## gRPC GraphService

원본 정의는 `byoridb-graph/proto/graph.proto`에 있습니다.

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
  bool read_only = 3;
}

message ExecuteResponse {
  int32 error_code = 1;
  string error_msg = 2;
  int64 latency_us = 3;
  bytes data = 4 [deprecated = true];
  DataSet result = 5;
}
```

Protobuf session ID는 `int64`를 유지하므로 protobuf client는 JavaScript JSON
number 정밀도 문제 없이 값을 보존할 수 있습니다.

현재 application error code는 다음과 같습니다.

| RPC | 코드 | 의미 |
|---|---:|---|
| `Authenticate` | `0` | 성공 |
| `Authenticate` | `1` | 인증 실패(`Invalid credentials`) |
| `Authenticate` | `3` | 로그인 제한; 응답에 안내된 기간 뒤 재시도 |
| `SignOut` | `0` | 성공 |
| `SignOut` | `1` | 그 밖의 sign-out 실패 |
| `SignOut` | `2` | Session이 없거나 만료됨 |
| `Execute` / `ExecuteJson` | `0` | 성공 |
| `Execute` / `ExecuteJson` | `1` | Query, parse, authorization 또는 실행 오류 |
| `Execute` / `ExecuteJson` | `2` | Session이 없거나 만료됨 |

### 결과 표현

`ExecuteResponse.result`가 권장 structured result이며 column 이름과 row를 담습니다.
Boolean, integer, float, string, null은 protobuf 전용 variant로 표현합니다. 복합
ByoriDB 값은 현재 `json_value` string fallback을 사용합니다. Deprecated `data`에는
이전 client를 위해 legacy result를 JSON으로 인코딩한 값이 들어갑니다.

서버는 gzip과 zstd로 압축된 gRPC request를 받으며 협상에 따라 gzip 또는 zstd
response를 보낼 수 있습니다. 수신 후 디코딩된 gRPC message의 상한은 64 MiB입니다.

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

`execute_raw`는 프로그램에서 structured protobuf 결과를 받는 경로입니다.
`execute`는 text table 형태를 렌더링하고 `execute_json`은 `serde_json::Value`를
반환합니다.

## HTTP endpoint

| Method | Path | 인증 | 용도 |
|---|---|---|---|
| `GET` | `/health` | 없음 | 프로세스 liveness handler |
| `GET` | `/ready` | 없음 | Query 수락 readiness |
| `GET` | `/metrics` | 없음 | Prometheus text metrics |
| `GET` | `/api/v1/metrics` | 없음 | Metrics discovery JSON |
| `GET` | `/api/v1/diagnostics/queries` | `X-ByoriDB-Session-Id`의 GOD/ADMIN session | 실행 중 query |
| `POST` | `/api/v1/session` | Body의 username/password | 인증 |
| `DELETE` | `/api/v1/session` | `X-ByoriDB-Session-Id`의 session | 해당 session 종료 |
| `POST` | `/api/v1/query` | Body의 session ID | 실행 후 JSON object 반환 |
| `POST` | `/api/v1/query/json` | Body의 session ID | 실행 후 raw JSON text 반환 |

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

무작위 63-bit 값 대부분은 JavaScript `Number`로 정확하게 표현할 수 없으므로 HTTP는
session ID를 **10진수 문자열**로 직렬화합니다. Query request는 호환성을 위해
10진수 문자열과 JSON 정수를 모두 허용하지만 client는 문자열을 보내야 합니다.

### Query 실행

```bash
curl -sS http://127.0.0.1:19669/api/v1/query \
  -H 'Content-Type: application/json' \
  --data '{
    "session_id": "734817462937615829",
    "query": "SHOW SPACES"
  }'
```

응답 형태 예시입니다. Metadata 값은 database에 따라 달라집니다.

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

`/api/v1/query/json`도 같은 요청을 받으며 같은 field를 raw JSON text로 직렬화합니다.
HTTP query 문자열의 상한은 1 MiB이며 초과하면 HTTP 413을 반환합니다.

### 읽기 전용 요청

두 query route는 선택적인 `read_only` flag를 받습니다. gRPC `ExecuteRequest`에도
같은 field가 있습니다.

```bash
curl -sS http://127.0.0.1:19669/api/v1/query \
  -H 'Content-Type: application/json' \
  -d '{"session_id": "1234", "query": "MATCH (p:person) RETURN p LIMIT 10", "read_only": true}'
```

읽기 전용 요청은 실행할 모든 절이 읽기인 경우에만 허용되며 그렇지 않으면
`403 PERMISSION_DENIED`로 거부합니다. `SHOW USERS`, `SHOW ROLES`, `SHOW SESSIONS`도
읽기 결과를 반환하지만 관리 명령이므로 거부합니다. 일반 요청에서 이 명령들은 `GOD`
또는 `ADMIN`을 요구합니다.

이 flag는 caller가 직접 작성하지 않은 문장, 즉 최종 사용자가 입력하거나 모델이 생성한
문장을 쓰기 권한이 있는 session으로 실행하면서 그 문장에 session의 쓰기 권한을 주지
않기 위한 것입니다. 서버가 compound statement와 `PROFILE`을 펼친 뒤 각 절을
검사하므로 caller가 세미콜론, 주석, pipeline을 별도로 금지할 필요가 없습니다.

- `SHOW SPACES; DELETE VERTEX 1`은 읽기로 시작해도 거부합니다.
- `PROFILE INSERT ...`는 내부 문장을 실행하므로 거부합니다. 일반 `EXPLAIN`은
  실행하지 않으므로 허용합니다.
- `SHOW SPACES; SHOW TAGS`는 모든 절이 읽기이므로 허용합니다.

Flag는 **요청 하나**만 제한하며 이후 요청에 영향을 남기지 않습니다. 같은 session은
다음 요청에서 쓸 수 있습니다. Flag를 생략하면 기존 동작을 유지합니다.

이것은 tenant나 credential 경계가 **아닙니다.** 기본 role은 모든 space에 적용되므로
읽기 전용 요청도 해당 session이 읽을 수 있는 모든 space를 읽습니다. 권한 모델이
격리하는 범위는 [보안](../operations/security.md)을 참고하세요.

Session 및 `/api/v1/query` route의 일반적인 JSON 오류 응답은 다음과 같습니다.

| Route | Status | Code | 조건 |
|---|---:|---|---|
| `/api/v1/query` | `400` | `QUERY_ERROR` | Parse, planning 또는 실행 실패 |
| `/api/v1/session` | `401` | `AUTH_FAILED` | Credential을 검증한 뒤 거부함 |
| `/api/v1/session` | `429` | `TOO_MANY_ATTEMPTS` | Credential 검증 전에 거부; `Retry-After` 포함 |
| `/api/v1/query` | `401` | `SESSION_EXPIRED` | Query session이 없거나 만료됨 |
| `/api/v1/query` | `403` | `PERMISSION_DENIED` | 인증됐지만 해당 role로 실행할 수 없음 |
| `/api/v1/query` | `413` | `QUERY_TOO_LARGE` | Query 문자열이 1 MiB 초과 |

Client가 status로 다음 동작을 결정할 수 있도록 실패 유형을 구분합니다.

- **`401`**: `/api/v1/query`에서는 session이 사라졌으므로 다시 인증하고 `USE`로 space를
  다시 선택합니다. 새 session은 선택된 space 없이 시작합니다. `/api/v1/session`에서는
  credential 자체가 거부됐으므로 같은 값으로 다시 시도하면 실패하고 계정의 실패 한도를
  소비합니다.
- **`403`**: Session은 계속 유효합니다. 재인증은 권한을 바꾸지 않으므로 role과 요청의
  `read_only` flag를 확인하세요.
- **`429`**: Credential을 검사하지 않았습니다. `Retry-After`만큼 기다린 뒤 같은
  credential로 재시도하세요. [로그인 제한](#login-throttling)을 참고하세요.
- **`400`**: Parsing, planning 또는 실행이 실패했습니다. 재시도 여부를 결정하기 전에
  오류와 서버 상태를 확인하세요.

과거 authorization failure는 `Authentication failed:`로 시작하는 text와 함께
`400 QUERY_ERROR`로 보고됐습니다. Error text는 안정된 machine interface가 아니므로
문자열로 분류하지 말고 status와 code를 사용하세요.

`/api/v1/query/json`은 raw JSON text 안에 같은 status와 code를 반환합니다. 단 HTTP
413 응답은 `QUERY_TOO_LARGE` code 없이 plain string으로 반환합니다.

### Session 종료

```bash
curl -sS -X DELETE \
  http://127.0.0.1:19669/api/v1/session \
  -H 'X-ByoriDB-Session-Id: 734817462937615829'
```

Endpoint는 header로 식별한 session을 검증하고 종료합니다. Header가 없거나 형식이
잘못되면 `AUTH_REQUIRED`와 HTTP 401을, 파싱된 session이 만료됐거나 존재하지 않으면
`SESSION_EXPIRED`와 HTTP 401을 반환합니다.

### 실행 중 query

```bash
curl -sS http://127.0.0.1:19669/api/v1/diagnostics/queries \
  -H 'X-ByoriDB-Session-Id: 734817462937615829'
```

유효한 session header가 없으면 HTTP 401을, `GOD` 또는 `ADMIN`이 아니면 403을
반환합니다. 결과는 `id`, `query_type`, `query_length_bytes`, `space`,
`started_at_ms`만 포함하며 raw query text와 session ID는 제외합니다.

## 현재 제한과 호환성

- 로그인 실패 제한과 동시 실행 수가 제한된 Argon2 검증은 내장되어 있습니다. 일반적인
  IP별 connection 또는 query QPS 제한은 없으므로 proxy나 network edge에서 적용하세요.
- HTTP와 gRPC endpoint는 현재 path/protobuf definition으로만 version을 구분합니다.
  Client upgrade 전에 release 변경을 검토하세요.
- HTTP session ID는 출력 시 문자열이며 gRPC는 protobuf `int64`를 사용합니다.
- Session은 영속적이거나 cluster 전체에 공유되는 상태가 아닙니다.
- Health와 metrics route는 인증되지 않으므로 network에서 접근을 제한해야 합니다.

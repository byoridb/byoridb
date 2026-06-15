# API 레퍼런스

ByoriDB는 gRPC와 HTTP API를 제공합니다.

## 연결

### 기본 포트

| 서비스 | gRPC 포트 | HTTP 포트 |
|---------|-----------|-----------|
| Graph | 9669 | 19669 |
| Meta | 9559 | - |
| Storage | 9779 | - |

### 인증

ByoriDB는 시작 시 `root` 슈퍼유저를 생성합니다. 재시작 후에도 비밀번호를 동일하게
유지하려면 서버를 시작하기 전에 `BYORIDB_ROOT_PASSWORD`를 설정하세요. 이 변수가
없으면 서버가 무작위 비밀번호를 생성하고 한 번 로그에 출력합니다.

## gRPC API

### 서비스 정의

```protobuf
service GraphService {
    rpc Execute(ExecuteRequest) returns (ExecuteResponse);
    rpc ExecuteJson(ExecuteJsonRequest) returns (ExecuteJsonResponse);
}

message ExecuteRequest {
    bytes session_id = 1;
    string statement = 2;
}

message ExecuteResponse {
    ErrorCode error_code = 1;
    string error_msg = 2;
    DataSet data = 3;
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
| `/metrics` | GET | Prometheus 지표 |
| `/api/v1/session` | POST | 인증된 세션 생성 |
| `/api/v1/session/{id}` | DELETE | 세션 종료 |
| `/api/v1/query` | POST | 쿼리 실행 |
| `/api/v1/query/json` | POST | 쿼리를 실행하고 JSON 반환 |

### 세션 생성

```bash
curl -X POST http://localhost:19669/api/v1/session \
  -H "Content-Type: application/json" \
  -d '{
    "username": "root",
    "password": "change-me-before-production"
  }'
```

### 쿼리 실행

```bash
curl -X POST http://localhost:19669/api/v1/query \
  -H "Content-Type: application/json" \
  -d '{
    "session_id": 1,
    "query": "SHOW SPACES"
  }'
```

응답:

```json
{
  "columns": ["Name"],
  "rows": [["my_space"], ["test_space"]]
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
# HELP byoridb_query_total Total queries
# TYPE byoridb_query_total counter
byoridb_query_total{type="read"} 1234
byoridb_query_total{type="write"} 567
```

## 에러 코드

| 코드 | 이름 | 설명 |
|------|------|-------------|
| 0 | SUCCEEDED | 작업 성공 |
| -1 | E_DISCONNECTED | 클라이언트 연결 끊김 |
| -2 | E_FAIL_TO_CONNECT | 연결 실패 |
| -3 | E_RPC_FAILURE | RPC 오류 |
| -4 | E_SESSION_INVALID | 유효하지 않은 세션 |
| -5 | E_SESSION_TIMEOUT | 세션 만료 |
| -6 | E_SYNTAX_ERROR | 쿼리 구문 오류 |
| -7 | E_SEMANTIC_ERROR | 쿼리 의미 오류 |
| -8 | E_EXECUTION_ERROR | 쿼리 실행 실패 |
| -9 | E_SPACE_NOT_FOUND | Space를 찾을 수 없음 |
| -10 | E_TAG_NOT_FOUND | Tag를 찾을 수 없음 |
| -11 | E_EDGE_NOT_FOUND | Edge 타입을 찾을 수 없음 |
| -12 | E_VERTEX_NOT_FOUND | Vertex를 찾을 수 없음 |
| -13 | E_INDEX_NOT_FOUND | Index를 찾을 수 없음 |
| -14 | E_USER_NOT_FOUND | 사용자를 찾을 수 없음 |
| -15 | E_BAD_USERNAME_PASSWORD | 인증 실패 |

## 데이터 타입

### Protocol Buffer 타입

```protobuf
message Value {
    oneof value {
        bool bool_val = 1;
        int64 int_val = 2;
        double float_val = 3;
        string str_val = 4;
        Date date_val = 5;
        Time time_val = 6;
        DateTime datetime_val = 7;
        Vertex vertex_val = 8;
        Edge edge_val = 9;
        Path path_val = 10;
        List list_val = 11;
        Map map_val = 12;
    }
}

message Vertex {
    int64 vid = 1;
    repeated Tag tags = 2;
}

message Edge {
    int64 src = 1;
    int64 dst = 2;
    int32 type = 3;
    string name = 4;
    int64 ranking = 5;
    map<string, Value> props = 6;
}
```

### JSON 타입

| nGQL 타입 | JSON 타입 |
|-----------|-----------|
| BOOL | boolean |
| INT8/16/32/64 | number |
| FLOAT/DOUBLE | number |
| STRING | string |
| DATE | string (ISO 8601) |
| DATETIME | string (ISO 8601) |
| LIST | array |
| MAP | object |

## 속도 제한(Rate Limiting)

기본 제한값:

| 제한 항목 | 값 |
|-------|-------|
| IP당 최대 연결 수 | 100 |
| 초당 최대 쿼리 수 | 1000 |
| 최대 쿼리 크기 | 4 MB |
| 쿼리 타임아웃 | 300초 |

제한값 설정:

```toml
[limits]
max_connections_per_ip = 100
max_queries_per_second = 1000
max_query_size_bytes = 4194304
query_timeout_secs = 300
```

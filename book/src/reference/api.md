# API Reference

ByoriDB provides gRPC and HTTP APIs.

## Connection

### Default Ports

| Service | gRPC Port | HTTP Port |
|---------|-----------|-----------|
| Graph | 9669 | 19669 |
| Meta | 9559 | - |
| Storage | 9779 | - |

### Authentication

ByoriDB creates a `root` superuser on startup. Set `BYORIDB_ROOT_PASSWORD`
before starting the server to make the password stable across restarts. If the
variable is absent, the server generates a random password and logs it once.

## gRPC API

### Service Definition

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

### Client Connection

```rust
use byoridb_client::Client;

let mut client = Client::connect(
    "localhost:9669".to_string(),
    "root".to_string(),
    std::env::var("BYORIDB_ROOT_PASSWORD")?,
).await?;

let result = client.execute("SHOW SPACES").await?;
```

### Session Management

Sessions are created during authentication. The Rust client manages the session
ID internally after `Client::connect`.

## HTTP API

### Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Health check |
| `/metrics` | GET | Prometheus metrics |
| `/api/v1/session` | POST | Create authenticated session |
| `/api/v1/session/{id}` | DELETE | Close session |
| `/api/v1/query` | POST | Execute query |
| `/api/v1/query/json` | POST | Execute query and return JSON |

### Create Session

```bash
curl -X POST http://localhost:19669/api/v1/session \
  -H "Content-Type: application/json" \
  -d '{
    "username": "root",
    "password": "change-me-before-production"
  }'
```

### Execute Query

```bash
curl -X POST http://localhost:19669/api/v1/query \
  -H "Content-Type: application/json" \
  -d '{
    "session_id": 1,
    "query": "SHOW SPACES"
  }'
```

Response:

```json
{
  "columns": ["Name"],
  "rows": [["my_space"], ["test_space"]]
}
```

### Health Check

```bash
curl http://localhost:19669/health

# Response
OK
```

### Metrics

```bash
curl http://localhost:19669/metrics

# Response (Prometheus format)
# HELP byoridb_query_total Total queries
# TYPE byoridb_query_total counter
byoridb_query_total{type="read"} 1234
byoridb_query_total{type="write"} 567
```

## Error Codes

| Code | Name | Description |
|------|------|-------------|
| 0 | SUCCEEDED | Operation successful |
| -1 | E_DISCONNECTED | Client disconnected |
| -2 | E_FAIL_TO_CONNECT | Connection failed |
| -3 | E_RPC_FAILURE | RPC error |
| -4 | E_SESSION_INVALID | Invalid session |
| -5 | E_SESSION_TIMEOUT | Session expired |
| -6 | E_SYNTAX_ERROR | Query syntax error |
| -7 | E_SEMANTIC_ERROR | Query semantic error |
| -8 | E_EXECUTION_ERROR | Query execution failed |
| -9 | E_SPACE_NOT_FOUND | Space not found |
| -10 | E_TAG_NOT_FOUND | Tag not found |
| -11 | E_EDGE_NOT_FOUND | Edge type not found |
| -12 | E_VERTEX_NOT_FOUND | Vertex not found |
| -13 | E_INDEX_NOT_FOUND | Index not found |
| -14 | E_USER_NOT_FOUND | User not found |
| -15 | E_BAD_USERNAME_PASSWORD | Authentication failed |

## Data Types

### Protocol Buffer Types

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

### JSON Types

| nGQL Type | JSON Type |
|-----------|-----------|
| BOOL | boolean |
| INT8/16/32/64 | number |
| FLOAT/DOUBLE | number |
| STRING | string |
| DATE | string (ISO 8601) |
| DATETIME | string (ISO 8601) |
| LIST | array |
| MAP | object |

## Rate Limiting

Default limits:

| Limit | Value |
|-------|-------|
| Max connections per IP | 100 |
| Max queries per second | 1000 |
| Max query size | 4 MB |
| Query timeout | 300 seconds |

Configure limits:

```toml
[limits]
max_connections_per_ip = 100
max_queries_per_second = 1000
max_query_size_bytes = 4194304
query_timeout_secs = 300
```

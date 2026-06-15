# 설정

ByoriDB는 명령줄 인자 또는 설정 파일을 통해 구성할 수 있습니다.

## 명령줄 옵션

### 서버

서버는 `byoridb.toml`과 `BYORIDB__...` 환경변수를 읽습니다.

### CLI 옵션

```bash
byoridb-cli [OPTIONS]

Options:
  --addr <ADDR>          Server address (default: 127.0.0.1:9669)
  --user <USER>          Username (required, env: BYORIDB_USER)
  --password <PASS>      Password (required, env: BYORIDB_PASSWORD)
  --execute <QUERY>      Execute a single query and exit
```

## 설정 파일

`byoridb.toml` 파일을 생성합니다:

```toml
[server]
graph_addr = "0.0.0.0:9669"
http_addr = "0.0.0.0:19669"
storage_addr = "0.0.0.0:44500"

[storage]
data_paths = ["/var/lib/byoridb/storage"]
```

## 환경변수

| 변수 | 설명 | 기본값 |
|----------|-------------|---------|
| `BYORIDB__SERVER__GRAPH_ADDR` | gRPC 수신 주소 | `0.0.0.0:9669` |
| `BYORIDB__SERVER__HTTP_ADDR` | HTTP 수신 주소 | `0.0.0.0:19669` |
| `BYORIDB__SERVER__STORAGE_ADDR` | 스토리지 서비스 수신 주소 | `0.0.0.0:44500` |
| `BYORIDB__STORAGE__DATA_PATHS` | 스토리지 데이터 경로 | `data/storage` |
| `BYORIDB_ROOT_PASSWORD` | Root 사용자 비밀번호 | 설정하지 않으면 생성되어 한 번만 로그에 출력됨 |
| `BYORIDB_USER` | CLI 사용자명 | 없음 |
| `BYORIDB_PASSWORD` | CLI 비밀번호 | 없음 |

## Root 사용자

`root` 사용자는 항상 생성됩니다. 알려진 비밀번호를 사용하려면 시작 전에
`BYORIDB_ROOT_PASSWORD`를 설정하세요. 설정하지 않으면 서버가 생성된 비밀번호를 한 번 로그에 출력합니다.

## 디렉터리 구조

서버를 시작한 후:

```
data/
├── meta/          # Metadata storage
├── storage/       # Graph data storage
└── wal/           # Write-ahead logs
```

## 성능 튜닝

### 메모리 설정

고성능 워크로드의 경우:

```toml
[storage]
block_cache_size = "1GB"
write_buffer_size = "128MB"
max_write_buffer_number = 4
```

### 압축

디스크 사용량을 줄이려면 압축을 활성화합니다:

```toml
[storage]
compression = "lz4"  # Options: none, snappy, lz4, zstd
```

## 로깅

ByoriDB는 구조적 로깅(structured logging)을 사용합니다. 로그 출력을 설정하려면:

```bash
# JSON format
BYORIDB_LOG_FORMAT=json ./byoridb-server

# Enable debug logs for specific modules
RUST_LOG=byoridb_graph=debug,byoridb_storage=info ./byoridb-server
```

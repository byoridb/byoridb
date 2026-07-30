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

[storage]
data_paths = ["/var/lib/byoridb/storage"]
```

## 환경변수

| 변수 | 설명 | 기본값 |
|----------|-------------|---------|
| `BYORIDB__SERVER__GRAPH_ADDR` | gRPC 수신 주소 | `0.0.0.0:9669` |
| `BYORIDB__SERVER__HTTP_ADDR` | HTTP 수신 주소 | `0.0.0.0:19669` |
| `BYORIDB__SERVER__STORAGE_ADDR` | 예약된 설정; standalone launcher에서는 아직 사용하지 않음 | `0.0.0.0:44500` |
| `BYORIDB__STORAGE__DATA_PATHS` | 스토리지 데이터 경로 | `data/storage` |
| `BYORIDB_ROOT_PASSWORD` | Root 사용자 비밀번호 | 필수(network server) |
| `BYORIDB_CACHE_SIZE_MB` | redb page cache 크기(MiB) | `256` |
| `BYORIDB_MAX_MEMORY_MB` | 쿼리 결과 메모리 soft cap(MiB) | `1024` |
| `BYORIDB_MAX_SCAN_LIMIT` | prefix scan 기본 최대 행 수 | `100000` |
| `BYORIDB_DURABILITY` | `immediate` 또는 bulk-load용 `relaxed` | `immediate` |
| `BYORIDB_USER` | CLI 사용자명 | 없음 |
| `BYORIDB_PASSWORD` | CLI 비밀번호 | 없음 |

## Root 사용자

`root` 사용자는 항상 생성됩니다. network server는 시작 전에
`BYORIDB_ROOT_PASSWORD`가 없거나 빈 값이면 시작을 거부합니다. credential은 로그에
출력되지 않으므로 secret manager에서 주입하세요.

## 디렉터리 구조

서버를 시작한 후:

```
data/
└── storage/
    └── data.redb  # current view + bitemporal history
```

## 성능 튜닝

### 메모리 설정

read-heavy 워크로드에서는 `BYORIDB_CACHE_SIZE_MB`를 working set과 노드 메모리에 맞춰
조정합니다. `BYORIDB_DURABILITY=relaxed`는 commit마다 fsync하지 않아 최근 commit을
잃을 수 있으므로 재적재 가능한 bulk import에만 사용하고 steady-state server에서는
기본 `immediate`를 유지하세요. redb에는 RocksDB식 write buffer/compression 설정이 없습니다.

## 로깅

standalone server는 `tracing_subscriber`의 text formatter로 stdout에 로그를 출력하고
`RUST_LOG`로 레벨과 모듈 필터를 설정합니다. 현재 launcher는
`BYORIDB_LOG_FORMAT`을 읽지 않으므로 JSON 출력 전환은 지원하지 않습니다.

```bash
# Enable debug logs for specific modules
BYORIDB_ROOT_PASSWORD='<root-password>' \
  RUST_LOG=byoridb_graph=debug,byoridb_storage=info ./byoridb-server
```

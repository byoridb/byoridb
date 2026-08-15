# 설정

[English](../../getting-started/configuration.html) | **한국어**

standalone server는 built-in default, working directory의 선택적 `byoridb.toml`,
`BYORIDB__...` 환경변수 순서로 설정을 읽습니다. server 설정용 명령줄 flag는
없습니다. server는 `--version`과 `--help`만 받고, 그 외 인자는 거부합니다:

```bash
$ byoridb-server --version
byoridb-server 0.3.3 (commit 9200800a1b2c, release)

$ byoridb-server --help    # 아래의 모든 key와 환경변수를 출력합니다
```

`--version`은 해당 binary가 어느 commit에서 build되었는지 보고합니다. 유지되는
semver release line이 아직 없으므로, 배포된 artifact는 이 값으로 식별합니다.
수정된 working tree에서 build한 경우 `-dirty`가 붙고, git checkout 밖에서
build한 경우 `unknown`으로 보고합니다. 두 flag 모두 credential을 읽거나 storage를
열거나 listener를 bind하지 않고 종료하므로, 설치된 binary에 언제든 안전하게
실행할 수 있습니다.

CLI 연결 설정에는 아래 명령줄 옵션을 사용할 수 있습니다.

## CLI 옵션

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

`BYORIDB__STORAGE__DATA_PATHS`는 comma-separated 값을 받을 수 있지만 현재 storage는
첫 번째 경로만 엽니다. 나머지 값은 striping이나 failover를 제공하지 않습니다.

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

HTTP와 gRPC listener에는 native TLS와 network-level login rate limiter가 없습니다.
비로컬 배포에서는 trusted proxy/ingress에서 TLS와 rate limit을 적용하고 listener 접근을
방화벽이나 network policy로 제한하세요.

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

## 로그인 throttling

실패한 로그인은 계정별·출처 주소별 budget과 lockout으로 제한됩니다. 성공한 로그인은
throttle되지 않고 동시 로그인 수에도 상한이 없습니다 — wire 계약은
[Login throttling](../reference/api.md#login-throttling)을 참고하세요. 기본값은
외부에 노출된 listener에 적합한 값입니다:

```toml
[auth]
login_window_secs = 60
max_account_failures_per_window = 20
max_source_failures_per_window = 60
max_concurrent_verifications = 4
max_failed_attempts = 5
lockout_duration_secs = 300
```

| 키 | 환경변수 | 기본값 | 의미 |
|---|---|---|---|
| `auth.login_window_secs` | `BYORIDB__AUTH__LOGIN_WINDOW_SECS` | `60` | 실패를 집계하는 sliding window |
| `auth.max_account_failures_per_window` | `BYORIDB__AUTH__MAX_ACCOUNT_FAILURES_PER_WINDOW` | `20` | username당 window 내 허용 실패 수 |
| `auth.max_source_failures_per_window` | `BYORIDB__AUTH__MAX_SOURCE_FAILURES_PER_WINDOW` | `60` | peer 주소당 window 내 허용 실패 수 |
| `auth.max_concurrent_verifications` | `BYORIDB__AUTH__MAX_CONCURRENT_VERIFICATIONS` | `4` | 동시 Argon2 검증 수; 초과분은 대기 |
| `auth.max_failed_attempts` | `BYORIDB__AUTH__MAX_FAILED_ATTEMPTS` | `5` | 존재하는 계정을 잠그는 연속 실패 수 |
| `auth.lockout_duration_secs` | `BYORIDB__AUTH__LOCKOUT_DURATION_SECS` | `300` | 그 lockout의 지속 시간; `0`이면 lockout 비활성화 |

> **이 값을 완화하는 것은 network 경계에서 접근이 제한된 listener에서만 안전합니다.**
> 엔진이 credential 추측에 대해 가진 유일한 방어이고, 대체할 다른 rate limiter가
> 없습니다. bind 주소로부터 자동 완화되는 것은 아무것도 없습니다 — `127.0.0.1`에
> bind해도 완화되지 않습니다. 프로세스는 단일 사용자 데스크톱과 forwarded port로
> 도달 가능한 호스트를 구분할 수 없기 때문입니다. 판단과 기록은 운영자의 몫입니다.

이 설정이 존재하는 이유는 단일 사용자 배포입니다 — 비밀번호를 잘못 입력하면 유일한
계정이 잠기고, 복구해 줄 두 번째 관리자가 없는 상황입니다. lockout만 비활성화하면
window budget은 그대로 남아 추측을 계속 제한합니다:

```bash
export BYORIDB__AUTH__LOCKOUT_DURATION_SECS=0
```

모든 로그인을 거부하게 만드는 값은 조용히 clamp하지 않고 **시작 시 거부**합니다.
window가 0, 계정·출처 budget이 0, 검증 permit이 0, lockout 임계값이 0인 경우 모두
`[auth]`를 명시한 오류와 함께 로드에 실패합니다. lockout을 끄려면
`lockout_duration_secs = 0`을 쓰세요. `max_failed_attempts = 0`은 그 표현이 아니라
오류입니다.

`max_concurrent_verifications`는 세션 수가 아니라 CPU 비용을 제한합니다. 초과한
로그인은 거부되지 않고 permit을 기다리므로, 값을 낮추면 burst가 느려질 뿐 정상
비밀번호가 실패로 바뀌지 않습니다.

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

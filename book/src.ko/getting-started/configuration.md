[English](../../getting-started/configuration.html)

# 설정

독립 실행형 서버는 내장 기본값, 작업 디렉터리의 선택적 `byoridb.toml` 파일,
`BYORIDB__...` 환경변수 순서로 설정을 읽습니다. 환경변수에서는 두 개의 밑줄로
설정 키의 계층을 구분합니다.

서버에는 이 설정들을 변경하는 명령줄 옵션이 없습니다. `--version`과 `--help`만
받으며, 그 밖의 인자는 거부합니다:

```bash
$ byoridb-server --version
byoridb-server 0.4.20 (commit c9bd11c39001, release)

$ byoridb-server --help    # lists every key and environment variable below
```

위 출력은 v0.4.20 릴리스를 기준으로 한 예시입니다. 설치된 바이너리는 자체 버전과
커밋을 표시합니다. `--version`은 태그가 없는 리비전까지 포함하여 정확한 빌드를
식별합니다. 수정된 작업 트리에서 빌드하면 `-dirty`가 붙고, Git 체크아웃 밖에서
빌드하면 `unknown`을 표시합니다. 두 옵션 모두 인증 정보를 읽거나 스토리지를
열거나 수신 소켓을 바인딩하지 않고 종료하므로 설치된 바이너리에 언제든 안전하게
실행할 수 있습니다.

## 최소 로컬 설정

개인 작업 환경에서만 접근할 서버는 루프백 주소에서 수신하도록 설정합니다:

```toml
# byoridb.toml
[server]
graph_addr = "127.0.0.1:9669"
http_addr = "127.0.0.1:19669"
storage_addr = "127.0.0.1:44500"

[storage]
data_paths = ["data/storage"]
```

`storage_addr`는 설정 모델에 남아 있지만, 현재 독립 실행형 실행기는 공개 Storage
gRPC 수신 소켓을 여는 대신 스토리지를 내장하여 실행합니다.

## 서버 설정 키

| 키 | 환경변수 | 기본값 |
| --- | --- | --- |
| `server.graph_addr` | `BYORIDB__SERVER__GRAPH_ADDR` | `0.0.0.0:9669` |
| `server.http_addr` | `BYORIDB__SERVER__HTTP_ADDR` | `0.0.0.0:19669` |
| `server.storage_addr` | `BYORIDB__SERVER__STORAGE_ADDR` | `0.0.0.0:44500` |
| `storage.data_paths` | `BYORIDB__STORAGE__DATA_PATHS` | `data/storage` |

파서는 쉼표로 구분한 환경변수 값으로 여러 데이터 경로를 받습니다:

```bash
export BYORIDB__STORAGE__DATA_PATHS='/data/one,/data/two'
```

현재 스토리지 환경은 첫 번째 경로만 엽니다. 나머지 경로는 설정에 보존되지만
데이터 분산 저장이나 장애 시 경로 전환을 제공하지 않습니다.

## 인증 정보

`BYORIDB_ROOT_PASSWORD`는 두 개의 밑줄로 구분하는 설정 트리와 별개입니다.
독립 실행형 바이너리를 시작하려면 비어 있지 않은 값을 설정해야 합니다.

```bash
export BYORIDB_ROOT_PASSWORD='value-from-your-secret-manager'
```

Root 인증 정보는 nGQL로 교체할 수 없습니다. 관리 중인 비밀 값을 변경한 다음
프로세스를 다시 시작하세요. 서버는 비밀번호를 로그에 남기지 않습니다.

CLI는 별도의 환경변수를 사용합니다:

| 변수 | 용도 |
| --- | --- |
| `BYORIDB_USER` | 필수 CLI 사용자 이름 |
| `BYORIDB_PASSWORD` | 필수 CLI 비밀번호 |

<a id="sessions-and-login-throttling"></a>

## 세션과 로그인 시도 제한

`[auth]`는 bearer 세션의 수명과 실패한 로그인 시도의 제한 방식을 설정합니다.
실패한 로그인은 계정별·출처 주소별 허용 횟수와 계정 잠금으로 제한됩니다.
성공한 로그인은 실패 허용 횟수를 소모하지 않습니다. 동시 Argon2 검증 제한을
초과한 요청은 대기하며, 이 제한은 세션 수를 제한하지 않습니다. 통신 규약은
[로그인 시도 제한](../reference/api.md#login-throttling)을 참고하세요.
기본값은 외부에 노출된 수신 소켓에 적합합니다:

```toml
[auth]
session_ttl_secs = 86400
login_window_secs = 60
max_account_failures_per_window = 20
max_source_failures_per_window = 60
max_concurrent_verifications = 4
max_failed_attempts = 5
lockout_duration_secs = 300
```

| 키 | 환경변수 | 기본값 | 의미 |
| --- | --- | --- | --- |
| `auth.session_ttl_secs` | `BYORIDB__AUTH__SESSION_TTL_SECS` | `86400` | Bearer 세션 수명; 사용할 때마다 갱신 |
| `auth.login_window_secs` | `BYORIDB__AUTH__LOGIN_WINDOW_SECS` | `60` | 실패 횟수를 집계하는 이동 시간 구간 |
| `auth.max_account_failures_per_window` | `BYORIDB__AUTH__MAX_ACCOUNT_FAILURES_PER_WINDOW` | `20` | 시간 구간 내 사용자 이름별 허용 실패 횟수 |
| `auth.max_source_failures_per_window` | `BYORIDB__AUTH__MAX_SOURCE_FAILURES_PER_WINDOW` | `60` | 시간 구간 내 접속 주소별 허용 실패 횟수 |
| `auth.max_concurrent_verifications` | `BYORIDB__AUTH__MAX_CONCURRENT_VERIFICATIONS` | `4` | 동시 Argon2 검증 수; 초과한 로그인은 대기 |
| `auth.max_failed_attempts` | `BYORIDB__AUTH__MAX_FAILED_ATTEMPTS` | `5` | 기존 계정을 잠그는 연속 실패 횟수 |
| `auth.lockout_duration_secs` | `BYORIDB__AUTH__LOCKOUT_DURATION_SECS` | `300` | 계정 잠금 지속 시간; `0`이면 잠금 비활성화 |

> **이 제한은 네트워크 경계에서 접근을 제한한 수신 소켓에서만 완화해야 합니다.**
> 이 설정은 인증 정보 추측에 대한 엔진의 유일한 방어 수단이며, 이를 대체할 다른
> 요청 제한 장치는 없습니다. 바인딩 주소에 따라 자동으로 완화되는 설정도 없습니다.
> `127.0.0.1`에 바인딩해도 제한은 그대로입니다. 프로세스는 단일 사용자 데스크톱과
> 포트 전달로 접근할 수 있는 호스트를 구분할 수 없기 때문입니다. 운영자가 판단하고
> 그 결정을 기록해야 합니다.

이 설정은 비밀번호를 잘못 입력해 유일한 계정이 잠겼지만, 복구를 도울 다른 관리자가
없는 단일 사용자 배포를 위해 마련되었습니다. 계정 잠금만 비활성화해도 시간 구간별
실패 허용 횟수는 유지되므로 인증 정보 추측을 계속 제한할 수 있습니다:

```bash
export BYORIDB__AUTH__LOCKOUT_DURATION_SECS=0
```

모든 로그인을 거부하게 만드는 값은 자동으로 보정하지 않고 **시작 시 거부**합니다.
집계 시간 구간, 계정별 또는 출처별 실패 허용 횟수, 동시 검증 허용 수, 계정 잠금
임계값, 세션 TTL 중 하나라도 0이면 `[auth]`를 명시한 오류와 함께 설정 로드에
실패합니다. 계정 잠금을 끄려면 `lockout_duration_secs = 0`을 사용하세요.
`max_failed_attempts = 0`은 잠금 비활성화 설정이 아니라 오류입니다.

`max_concurrent_verifications`는 세션 수가 아니라 CPU 비용을 제한합니다. 이 값을
초과한 로그인은 거부되지 않고 검증 차례를 기다립니다. 따라서 값을 낮추면 몰려든
요청의 처리가 느려질 뿐, 올바른 비밀번호가 이 제한 때문에 실패하지는 않습니다.

<a id="session-lifetime"></a>

### 세션 수명

`session_ttl_secs`는 **사용할 때마다 갱신**됩니다. 세션을 사용할 때마다 전체 TTL만큼
연장하므로, 세션의 총수명이 아니라 *유휴* 상태로 유지될 수 있는 시간을 제한합니다.
계속 사용 중인 세션은 자체적으로 만료되지 않습니다. TTL을 줄이면 방치된 세션의
노출 시간을 제한할 수 있습니다. 사용 중인 세션을 종료하려면
`DELETE /api/v1/session`을 사용합니다.

세션 ID는 소지자에게 접근 권한을 부여하는 bearer 인증 정보이고 두 수신 서버 모두
자체 TLS를 제공하지 않습니다. 따라서 프록시에서 TLS를 종료하는 배포에서는 TTL을
줄이는 것이 의미 있는 보안 통제입니다:

```bash
export BYORIDB__AUTH__SESSION_TTL_SECS=3600
```

허용 범위는 1초에서 1년입니다. 상한은 실수로 사실상 영구적인 bearer 인증 정보가
생기는 것을 막고, 흔한 단위 오류를 잡기 위해 존재합니다. 단위는 **초**이므로
`86400000`은 약 2.7년으로 조용히 적용되지 않고 거부됩니다.

하나의 설정이 세션을 보관하는 두 저장소에 모두 적용됩니다.
[인증과 세션](../reference/api.md#authentication-and-sessions)을 참고하세요.
두 저장소의 상태는 접근할 때마다 맞춰지므로 서로 다른 수명을 적용한다면 더 짧은
쪽이 실제 TTL이 됩니다.

## 런타임 튜닝

다음 환경변수는 실행 중인 프로세스가 직접 읽습니다:

| 변수 | 기본값 | 의미 |
| --- | --- | --- |
| `BYORIDB_CACHE_SIZE_MB` | `256` | redb 페이지 캐시 크기(MiB); 양수여야 함 |
| `BYORIDB_DURABILITY` | 즉시 영속성 | `none`, `relaxed`, `eventual`은 완화된 영속성을 선택 |
| `BYORIDB_MAX_MEMORY_MB` | `1024` | 쿼리별 구체화된 결과의 메모리 소프트 상한; `0`이면 비활성화 |
| `BYORIDB_MAX_SCAN_LIMIT` | `100000` | 대체 스캔 한 번의 최대 행 수; `0`이면 비활성화 |
| `RUST_LOG` | 구독자 기본값 | Rust tracing 필터, 예: `byoridb_graph=debug` |

완화된 영속성을 사용하면 비정상 종료 후 최근 커밋이 유실될 수 있습니다.
다시 적재할 수 있는 데이터에만 사용하고 일반 서비스 운영에는 사용하지 마세요.

## 클러스터 설정

설정 모델은 다음 값도 받습니다:

```toml
[cluster]
node_id = 1
peers = []
advertise_addr = "127.0.0.1:9559"
bootstrap = false
meta_addr = "0.0.0.0:9559"
```

대응하는 환경변수는 `BYORIDB__CLUSTER__NODE_ID`, `BYORIDB__CLUSTER__PEERS`,
`BYORIDB__CLUSTER__ADVERTISE_ADDR`, `BYORIDB__CLUSTER__BOOTSTRAP`,
`BYORIDB__CLUSTER__META_ADDR`입니다. 쉼표로 구분한 `peers` 값이 비어 있지 않으면
Meta 실행기가 활성화됩니다.

클러스터 시작 기능은 아직 완성되지 않았습니다. Storage/Raft 피어 초기화, 배포 구성
연결, 다중 노드 운영의 종단 간 검증이 완료되지 않았습니다. 현재 클러스터 설정이나
`docker-compose.yml`의 서비스 세 개를 운영 환경용 분산 배포로 간주하지 마세요.

## 네트워크 보안

gRPC와 HTTP 서버는 TLS를 직접 종료하지 않습니다. 내장 로그인 실패 제한과 동시
Argon2 검증 제한은 프로세스 단위이며, 일반 연결이나 쿼리 트래픽을 제한하지 않습니다.
로컬 환경 밖에서 배포할 때는 다음과 같이 구성하세요:

- 신뢰할 수 있는 인그레스 또는 프록시에서 TLS를 종료합니다.
- 방화벽이나 네트워크 정책으로 수신 소켓 접근을 제한합니다.
- 외부 진입점에 요청 횟수 제한을 적용합니다.
- 비밀 관리 서비스에서 `BYORIDB_ROOT_PASSWORD`를 주입합니다.
- 네트워크 접근 제어가 없으면 기본 `0.0.0.0` 수신 주소를 사용하지 않습니다.

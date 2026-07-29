[English](../../getting-started/configuration.html)

# 설정

스탠드얼론 서버는 내장 기본값, 작업 디렉터리의 선택적 `byoridb.toml`,
`BYORIDB__...` 환경변수 순으로 설정을 읽습니다. 환경변수의 설정 키 구분자는
이중 밑줄입니다.

현재 서버는 이 설정을 위한 명령줄 옵션을 제공하지 않습니다.

## 최소 로컬 설정

개발 장비에서만 접근할 서버는 루프백 리스너를 사용하세요.

```toml
# byoridb.toml
[server]
graph_addr = "127.0.0.1:9669"
http_addr = "127.0.0.1:19669"
storage_addr = "127.0.0.1:44500"

[storage]
data_paths = ["data/storage"]
```

`storage_addr`는 설정 모델에 남아 있지만 현재 스탠드얼론 런처는 외부 Storage
gRPC 리스너를 시작하지 않고 저장소를 프로세스에 내장합니다.

## 서버 설정 키

| 키 | 환경변수 | 기본값 |
| --- | --- | --- |
| `server.graph_addr` | `BYORIDB__SERVER__GRAPH_ADDR` | `0.0.0.0:9669` |
| `server.http_addr` | `BYORIDB__SERVER__HTTP_ADDR` | `0.0.0.0:19669` |
| `server.storage_addr` | `BYORIDB__SERVER__STORAGE_ADDR` | `0.0.0.0:44500` |
| `storage.data_paths` | `BYORIDB__STORAGE__DATA_PATHS` | `data/storage` |

설정 파서는 여러 데이터 경로를 쉼표로 구분한 환경변수 값으로 받습니다.

```bash
export BYORIDB__STORAGE__DATA_PATHS='/data/one,/data/two'
```

현재 저장소 환경은 첫 번째 경로만 엽니다. 추가 항목은 설정에 남지만 striping이나
failover를 제공하지 않습니다.

## 자격 증명

`BYORIDB_ROOT_PASSWORD`는 이중 밑줄 설정 트리와 별개입니다. 스탠드얼론
바이너리는 비어 있지 않은 값이 있어야 시작합니다.

```bash
export BYORIDB_ROOT_PASSWORD='value-from-your-secret-manager'
```

nGQL로 root 자격 증명을 교체할 수 없습니다. 관리 중인 시크릿을 바꾸고
프로세스를 재시작하세요. 서버는 비밀번호를 로그에 출력하지 않습니다.

CLI는 별도 변수를 사용합니다.

| 변수 | 용도 |
| --- | --- |
| `BYORIDB_USER` | 필수 CLI 사용자 이름 |
| `BYORIDB_PASSWORD` | 필수 CLI 비밀번호 |

## 런타임 튜닝

현재 프로세스가 직접 읽는 변수입니다.

| 변수 | 기본값 | 의미 |
| --- | --- | --- |
| `BYORIDB_CACHE_SIZE_MB` | `256` | redb 페이지 캐시 크기(MiB), 양수만 허용 |
| `BYORIDB_DURABILITY` | 즉시 내구성 | `none`, `relaxed`, `eventual`이면 완화된 내구성 사용 |
| `BYORIDB_MAX_MEMORY_MB` | `1024` | 쿼리별 결과 구체화 메모리 소프트 한도, `0`이면 비활성화 |
| `BYORIDB_MAX_SCAN_LIMIT` | `100000` | 한 번의 대체 스캔 최대 행 수, `0`이면 비활성화 |
| `RUST_LOG` | subscriber 기본값 | 예: `byoridb_graph=debug`인 Rust tracing 필터 |

완화된 내구성에서는 장애 시 최근 커밋을 잃을 수 있습니다. 다시 적재할 수 있는
데이터에만 사용하고 일반 서비스에는 사용하지 마세요.

## 클러스터 설정

설정 모델은 다음 값도 받습니다.

```toml
[cluster]
node_id = 1
peers = []
advertise_addr = "127.0.0.1:9559"
bootstrap = false
meta_addr = "0.0.0.0:9559"
```

대응 환경변수는 `BYORIDB__CLUSTER__NODE_ID`, `BYORIDB__CLUSTER__PEERS`,
`BYORIDB__CLUSTER__ADVERTISE_ADDR`, `BYORIDB__CLUSTER__BOOTSTRAP`,
`BYORIDB__CLUSTER__META_ADDR`입니다. 쉼표로 구분한 비어 있지 않은 `peers` 값은
Meta 런처를 활성화합니다.

클러스터 시작 경로는 아직 완성되지 않았습니다. Storage/Raft 피어 부트스트랩,
배포 연결, 다중 노드 운영 E2E가 닫히지 않았습니다. 현재 클러스터 스위치나
`docker-compose.yml`의 세 서비스를 프로덕션 분산 배포로 간주하지 마세요.

## 네트워크 보안

gRPC와 HTTP 서버는 TLS를 종료하지 않으며 내장 네트워크 수준 로그인 rate
limiter도 없습니다. 로컬 외부에 배포한다면 다음을 적용하세요.

- 신뢰할 수 있는 ingress 또는 프록시에서 TLS 종료
- 방화벽이나 네트워크 정책으로 리스너 접근 제한
- 경계에서 rate limiting 적용
- 시크릿 매니저에서 `BYORIDB_ROOT_PASSWORD` 주입
- 네트워크 통제 없이는 기본 `0.0.0.0` 리스너 사용 금지

# ByoriDB 빠른 시작

[English (기본)](QUICKSTART.md) | [한국어](QUICKSTART.ko.md)

이 가이드는 로컬 standalone 서버를 빌드하고 CLI와 HTTP API로 연결한 뒤 backup을
생성하는 과정을 설명합니다. standalone 단일 노드 운영이 주 지원 경로이며 현재
cluster launcher는 production-ready 상태가 아닙니다.

## 1. 사전 요구사항

- Linux 또는 macOS (native Windows는 현재 지원하지 않음)
- [rustup](https://rustup.rs/)으로 설치하고 `rust-toolchain.toml`에 고정된 Rust 1.90
- gRPC code generation을 위한 `protobuf-compiler` (`protoc`)

storage backend는 pure Rust(redb)이므로 C++ build toolchain이 필요하지 않습니다.

## 2. Workspace 빌드

현재 최신 공개 릴리스
[v0.3.3](https://github.com/byoridb/byoridb/releases/tag/v0.3.3)은 현재 `main`의
인증 강화, HTTP/gRPC session state 공유, temporal v1.1 변경 및 edge `AS OF`
조회보다 이전입니다. 아래 명령은 현재 checkout을 빌드합니다. 재현성이 중요하면
commit SHA를 고정하세요.

```bash
git clone https://github.com/byoridb/byoridb.git
cd byoridb
cargo build --locked --workspace --release
```

## 3. Standalone 서버 시작

binary를 시작하기 전에 강력한 root 비밀번호를 설정하세요.

```bash
export BYORIDB_ROOT_PASSWORD='replace-with-a-strong-local-secret'
cargo run --locked --release --bin byoridb-server
```

`BYORIDB_ROOT_PASSWORD`가 설정되지 않았거나 빈 값이면 `byoridb-server`는 fail
closed 방식으로 시작을 거부합니다. 검색할 수 있는 root 비밀번호를 자동 생성하거나
출력하지 않습니다. 배포 시에는 저장소, image 또는 commit 대상 `.env` 파일에 넣지
말고 secret manager로 주입하세요.

기본 listener:

| Protocol | 주소 | 용도 |
|---|---|---|
| gRPC | `0.0.0.0:9669` | Native client와 CLI |
| HTTP | `0.0.0.0:19669` | REST 스타일 session/query API와 metrics |

liveness와 readiness를 확인합니다.

```bash
curl --fail http://127.0.0.1:19669/health
# OK

curl --fail http://127.0.0.1:19669/ready
# READY
```

이 listener는 native TLS를 제공하지 않습니다. 로컬 환경 밖에서 사용할 때는
신뢰할 수 있는 network에 제한하거나 TLS termination 뒤에 배치하세요.

## 4. CLI 연결

다른 terminal을 열고 두 credential을 모두 명시합니다.

```bash
export BYORIDB_USER=root
export BYORIDB_PASSWORD='replace-with-a-strong-local-secret'
cargo run --locked -p byoridb-client --bin byoridb-cli
```

동일한 `--user`, `--password` flag도 있지만 password 환경 변수를 사용하면 secret이
shell history와 process argument에 직접 남는 것을 줄일 수 있습니다. CLI에는 기본
사용자나 비밀번호가 없습니다.

## 5. 기본 쿼리 실행

`byoridb>` prompt에서 space를 생성하고 선택합니다.

```sql
CREATE SPACE my_space(partition_num=10, replica_factor=1, vid_type=INT64);
USE my_space;
```

schema를 정의합니다.

```sql
CREATE TAG person(name STRING, age INT64);
CREATE EDGE follows(since INT64);
```

data를 삽입합니다.

```sql
INSERT VERTEX person(name, age) VALUES 100:("Tom", 20);
INSERT VERTEX person(name, age) VALUES 101:("Jerry", 22);
INSERT EDGE follows(since) VALUES 100->101:(2026);
```

조회합니다.

```sql
FETCH PROP ON person 100;
GO FROM 100 OVER follows;
LOOKUP ON person WHERE person.age > 20;
MATCH (a:person)-[e:follows]->(b:person) RETURN a, e, b;
```

asserted vertex/edge write에는 history가 기록됩니다. Application에서 확보한 epoch
millisecond timestamp를 현재의 point-in-time read 표면에 사용할 수 있습니다.

```sql
FETCH PROP ON person 100 AS OF <EPOCH_MS>;
FETCH PROP ON follows 100->101 AS OF <EPOCH_MS>;
```

`<EPOCH_MS>`를 application에서 확보한 point-in-time 값으로 교체하세요. Temporal
`MATCH`, temporal `GO`, `BETWEEN`, 사용자가 지정하는 `VALID FROM/TO`는 현재
지원하지 않습니다.

## 6. HTTP API 사용

### Session 생성

```bash
curl --fail-with-body -X POST http://127.0.0.1:19669/api/v1/session \
  -H 'Content-Type: application/json' \
  -d '{"username":"root","password":"replace-with-a-strong-local-secret"}'
```

응답 형태는 다음과 같습니다.

```json
{"session_id":"734214891234567890","time_zone":"UTC"}
```

대부분의 random 63-bit 값은 JavaScript `Number`로 정확하게 표현할 수 없으므로
session ID는 decimal JSON string으로 출력됩니다. String 그대로 유지하고 아래의
`<SESSION_ID>`를 응답값으로 교체하세요. Session ID는 bearer credential이므로
공개하거나 log에 기록하지 마세요.

### 쿼리 실행

```bash
curl --fail-with-body -X POST http://127.0.0.1:19669/api/v1/query \
  -H 'Content-Type: application/json' \
  -d '{"session_id":"<SESSION_ID>","query":"CREATE SPACE api_demo(partition_num=10, replica_factor=1)"}'

curl --fail-with-body -X POST http://127.0.0.1:19669/api/v1/query \
  -H 'Content-Type: application/json' \
  -d '{"session_id":"<SESSION_ID>","query":"USE api_demo"}'

curl --fail-with-body -X POST http://127.0.0.1:19669/api/v1/query \
  -H 'Content-Type: application/json' \
  -d '{"session_id":"<SESSION_ID>","query":"CREATE TAG person(name STRING, age INT64)"}'

curl --fail-with-body -X POST http://127.0.0.1:19669/api/v1/query \
  -H 'Content-Type: application/json' \
  -d '{"session_id":"<SESSION_ID>","query":"INSERT VERTEX person(name, age) VALUES 1:(\"Alice\", 30)"}'

curl --fail-with-body -X POST http://127.0.0.1:19669/api/v1/query \
  -H 'Content-Type: application/json' \
  -d '{"session_id":"<SESSION_ID>","query":"LOOKUP ON person"}'
```

1 MiB보다 큰 query string은 거부됩니다. `/api/v1/query/json`은 같은 query
operation의 응답을 raw JSON string으로 제공합니다.

### Metrics와 active query 확인

Prometheus metrics와 간단한 metrics descriptor에는 현재 인증이 적용되지 않습니다.

```bash
curl --fail http://127.0.0.1:19669/metrics
curl --fail http://127.0.0.1:19669/api/v1/metrics
```

active-query diagnostics는 live `GOD` 또는 `ADMIN` session을
`x-byoridb-session-id` header로 제공해야 합니다.

```bash
curl --fail-with-body http://127.0.0.1:19669/api/v1/diagnostics/queries \
  -H 'x-byoridb-session-id: <SESSION_ID>'
```

diagnostics는 raw session ID를 제외하고 password가 포함된 query text를 redact합니다.

### Sign out

```bash
curl --fail-with-body -X DELETE \
  http://127.0.0.1:19669/api/v1/session \
  -H 'x-byoridb-session-id: <SESSION_ID>'
```

Session ID는 URL과 response body에 포함되지 않습니다. 이 header를 bearer
credential로 취급하고 proxy와 application log에서 redact하도록 설정하세요.

## 7. 사용자와 role 이해

`root`에는 `GOD` role이 부여됩니다. User/role 관리, `SHOW USERS`, `SHOW ROLES`,
`SHOW SESSIONS`, `BALANCE`는 `GOD` 또는 `ADMIN`이 필요합니다.

```sql
CREATE USER reader WITH PASSWORD "replace-with-a-different-secret" ROLE GUEST;
GRANT ROLE USER TO reader;
REVOKE ROLE GUEST FROM reader;
ALTER USER reader WITH PASSWORD "replace-again";
DROP USER reader;
```

사용자의 비밀번호나 role 변경, 사용자 비활성화 또는 삭제는 현재 서버 프로세스에
있는 해당 사용자의 session을 무효화합니다. Live session state는 다른 프로세스에
분산되지 않습니다. `root` 계정은 이 statement로 생성, 삭제, 변경할 수 없습니다.
`BYORIDB_ROOT_PASSWORD`를 변경하고 서버를 재시작하여 비밀번호를 rotation하세요.

기본 role은 `GOD`, `ADMIN`, `DBA`, `USER`, `GUEST`입니다. 현재 권한은 wildcard
space `*`를 사용하며 space-scoped `GRANT` 문법은 없습니다. 이 role을 multi-tenant
isolation 경계로 사용하지 마세요. 전체 배포 model은
[SECURITY.ko.md](SECURITY.ko.md)를 참고하세요.

관리자용 `SHOW USERS` command(legacy alias `SHOW USER`)는 built-in root와 영속
사용자를 role과 함께 나열합니다. `SHOW ROLES`(alias `SHOW ROLE`)는 아직 별도의 role
catalog가 아니라 같은 user/role 목록을 반환합니다. `SHOW SESSIONS`는 active user와
선택된 space를 나열하지만 bearer session ID는 의도적으로 생략합니다.

## 8. 서버 설정

작업 directory에 선택적으로 `byoridb.toml`을 생성할 수 있습니다.

```toml
[server]
graph_addr = "127.0.0.1:9669"
http_addr = "127.0.0.1:19669"

[storage]
data_paths = ["data/storage"]
```

설정 환경 변수는 section과 key 사이에 double underscore를 사용합니다.

| 변수 | 기본값 | 의미 |
|---|---|---|
| `BYORIDB_ROOT_PASSWORD` | 없음, 필수 | Standalone root credential |
| `BYORIDB__SERVER__GRAPH_ADDR` | `0.0.0.0:9669` | gRPC listen 주소 |
| `BYORIDB__SERVER__HTTP_ADDR` | `0.0.0.0:19669` | HTTP listen 주소 |
| `BYORIDB__STORAGE__DATA_PATHS` | `data/storage` | 쉼표로 구분한 data directory |
| `BYORIDB_CACHE_SIZE_MB` | `256` | redb page-cache 크기(MiB) |
| `BYORIDB_DURABILITY` | immediate | 다시 load할 수 있는 bulk import에서만 `relaxed`, `none`, `eventual` 사용 |

relaxed durability는 commit마다 fsync하지 않으므로 crash 시 최근 commit이 손실될 수
있습니다. Steady-state serving에는 사용하지 마세요.

`BYORIDB__CLUSTER__*` 아래에 cluster 변수가 있지만 end-to-end multi-node 배포
경로는 완성되지 않았습니다. 지원되는 standalone 경로에서는 `cluster.peers`를
비워 두세요.

## 9. Backup과 restore

backup에는 current KV view와 temporal history가 모두 포함됩니다.

```bash
cargo run --locked --release --bin byoridb-backup -- create \
  --db data/storage \
  --backup-dir ./backups \
  --label daily

cargo run --locked --release --bin byoridb-backup -- list \
  --backup-dir ./backups

cargo run --locked --release --bin byoridb-backup -- verify \
  --backup-dir ./backups \
  --backup-id <BACKUP_ID>

cargo run --locked --release --bin byoridb-backup -- restore \
  --backup-dir ./backups \
  --backup-id <BACKUP_ID> \
  --target ./restored-data
```

`--overwrite`를 제공하지 않으면 restore는 기존 target을 교체하지 않습니다. Active
data directory를 교체하기 전에 restored database를 검증하세요.

## 다음 단계

- [아키텍처 개요](book/src.ko/architecture/overview.md)를 읽으세요.
- [nGQL 가이드](book/src.ko/guide/ngql-syntax.md)를 확인하세요.
- [보안 및 배포 가이드](SECURITY.ko.md)를 검토하세요.
- 변경을 제출하기 전에 [CONTRIBUTING.ko.md](CONTRIBUTING.ko.md)를 확인하세요.

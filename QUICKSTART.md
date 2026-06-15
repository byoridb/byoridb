# 빠른 시작 가이드

이 가이드는 ByoriDB를 설치하고 실행하는 데 도움을 줍니다.

## 사전 요구사항

- **Rust**: 최신 stable 버전 ([rustup](https://rustup.rs/)을 통해 설치)
- **Linux/macOS**: Windows는 현재 지원하지 않습니다
- C++ 빌드 도구가 필요 없습니다 — 스토리지는 순수 Rust(redb)로 구현되었습니다

## 1. 빌드 및 실행

프로젝트를 클론하고 빌드합니다:

```bash
git clone https://github.com/byoridb/byoridb.git
cd byoridb
cargo build --release
```

독립 실행형 서버(Meta, Storage, Graph 서비스 포함)를 시작합니다:

```bash
export BYORIDB_ROOT_PASSWORD='change-me-before-production'
cargo run --release --bin byoridb-server
```

기본 포트:
- **gRPC**: 9669
- **HTTP**: 19669

## 2. CLI 클라이언트로 연결하기

```bash
# 새 터미널에서
export BYORIDB_USER=root
export BYORIDB_PASSWORD='change-me-before-production'
cargo run -p byoridb-client --bin byoridb-cli
```

ByoriDB는 항상 `root` 사용자를 생성합니다. root 비밀번호는
`BYORIDB_ROOT_PASSWORD`에서 가져오며, 설정되지 않은 경우 서버가 무작위
비밀번호를 생성하여 시작 시 한 번 로그에 기록합니다.

## 3. 기본 쿼리 (CLI)

연결되면 다음 nGQL 쿼리를 실행해 보세요:

### Space 생성

```sql
CREATE SPACE my_space(partition_num=10, replica_factor=1, vid_type=INT64);
USE my_space;
```

### 스키마 정의

```sql
CREATE TAG person(name STRING, age INT64);
CREATE EDGE follow(degree INT64);
```

### 데이터 삽입

```sql
INSERT VERTEX person(name, age) VALUES 100:('Tom', 20);
INSERT VERTEX person(name, age) VALUES 101:('Jerry', 22);
INSERT EDGE follow(degree) VALUES 100->101:(95);
```

### 데이터 조회

```sql
FETCH PROP ON person 100;
GO FROM 100 OVER follow;
LOOKUP ON person WHERE person.age > 20;
```

## 4. HTTP REST API

HTTP API를 직접 사용할 수도 있습니다:

### 헬스 체크

```bash
curl http://localhost:19669/health
# OK
```

### 세션 생성

```bash
curl -X POST http://localhost:19669/api/v1/session \
  -H "Content-Type: application/json" \
  -d '{"username": "root", "password": "change-me-before-production"}'
# {"session_id":1,"time_zone":"UTC"}
```

### 쿼리 실행

```bash
# Space 생성
curl -X POST http://localhost:19669/api/v1/query \
  -H "Content-Type: application/json" \
  -d '{"session_id": 1, "query": "CREATE SPACE test(partition_num=10, replica_factor=1)"}'

# Space 사용
curl -X POST http://localhost:19669/api/v1/query \
  -H "Content-Type: application/json" \
  -d '{"session_id": 1, "query": "USE test"}'

# Tag 생성
curl -X POST http://localhost:19669/api/v1/query \
  -H "Content-Type: application/json" \
  -d '{"session_id": 1, "query": "CREATE TAG person(name STRING, age INT64)"}'

# Vertex 삽입
curl -X POST http://localhost:19669/api/v1/query \
  -H "Content-Type: application/json" \
  -d '{"session_id": 1, "query": "INSERT VERTEX person(name, age) VALUES 1:(\"Alice\", 30)"}'

# Lookup
curl -X POST http://localhost:19669/api/v1/query \
  -H "Content-Type: application/json" \
  -d '{"session_id": 1, "query": "LOOKUP ON person"}'
```

### Prometheus 메트릭

```bash
curl http://localhost:19669/metrics
```

## 5. 설정

### 환경 변수

| 변수 | 기본값 | 설명 |
|----------|---------|-------------|
| `BYORIDB__SERVER__GRAPH_ADDR` | `0.0.0.0:9669` | gRPC 서버 주소 |
| `BYORIDB__SERVER__HTTP_ADDR` | `0.0.0.0:19669` | HTTP 서버 주소 |
| `BYORIDB__STORAGE__DATA_PATHS` | `data/storage` | 데이터 디렉터리 |

### 설정 파일

`byoridb.toml`을 생성합니다:

```toml
[server]
graph_addr = "0.0.0.0:9669"
http_addr = "0.0.0.0:19669"

[storage]
data_paths = ["data/storage"]
```

설정과 함께 실행합니다:

```bash
cargo run --release --bin byoridb-server
```

## 6. 백업 및 복원

### 백업 생성

```bash
cargo run --release --bin byoridb-backup -- create \
  --db data/storage \
  --backup-dir /path/to/backups \
  --label "daily"
```

### 백업 복원

```bash
cargo run --release --bin byoridb-backup -- restore \
  --backup-dir /path/to/backups \
  -i backup_20240101_120000 \
  --target /path/to/restore
```

## 다음 단계

- [아키텍처 개요](book/src/architecture/overview.md)를 읽고 동작 방식을 이해하세요
- 전체 쿼리 레퍼런스는 [nGQL 문법](book/src/guide/ngql-syntax.md)을 확인하세요
- 프로젝트 개선에 참여하려면 [기여하기](CONTRIBUTING.md)를 참고하세요

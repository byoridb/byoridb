# ByoriDB

[English](../introduction.html)

ByoriDB는 Rust로 작성된 온톨로지 그래프 데이터베이스입니다. nGQL에서 영감을 받은
쿼리 언어, 영속 프로퍼티 그래프, RDFS-Plus 방식의 전방 추론, 추천 기능, 그리고
사용자가 기록한 정점과 엣지의 시점 조회를 제공합니다.

현재 지원하는 배포 방식은 **하나의 redb 데이터베이스를 사용하는 단일 ByoriDB
서버**입니다. 저장소에는 파티션 라우팅, Meta/Storage RPC, 커스텀 Raft 구성요소도
있지만 아직 운영 가능한 다중 노드 launcher로 완전히 연결되어 있지 않습니다.
클러스터 배포를 계획하기 전에 [분산 시스템](architecture/distributed.html)을
확인하세요.

## 구현된 기능

- **프로퍼티 그래프**: space, tag, edge type, vertex, edge, 보조 인덱스
- **쿼리**: `FETCH`, `GO`, `MATCH`, `LOOKUP`, `FIND PATH`, `EXPLAIN`, `PROFILE`
- **온톨로지 기능**: 클래스 계층, 시맨틱 edge 선언, 전방 materialization,
  `owl:sameAs`, `WHY` 추론 근거, 일관성 검사, SHACL 방식 shape 검증
- **추천**: 구조적 유사도, 임베딩 유사도, 큰 벡터 집합을 위한 영속 HNSW 인덱스,
  필터와 혼합 재랭킹
- **시간 조회**: DML이 현재 뷰와 asserted-fact 이력을 함께 관리하며,
  `FETCH PROP ... AS OF <epoch-ms>`로 정점과 엣지를 조회
- **인증과 권한**: 환경변수 기반 root 자격 증명, 영속 non-root 사용자, 기본 역할,
  세션 기반 gRPC/HTTP 접근, 쿼리 권한 검사
- **운영 기능**: Prometheus 지표, health/readiness endpoint, graceful shutdown,
  snapshot backup CLI, Docker 자산, 단일 replica AKS manifest

## 런타임 아키텍처

standalone 서버는 다음 crate를 한 프로세스에 조합합니다.

| Crate | 책임 |
|---|---|
| `byoridb-common` | 공통 그래프 값과 data set |
| `byoridb-kvstore` | redb 기반 현재/이력 keyspace |
| `byoridb-codec` | vertex, edge, value, row codec |
| `byoridb-storage` | storage environment, index, RPC, partition, Raft 구성요소 |
| `byoridb-meta` | schema, host, partition, migration metadata 구성요소 |
| `byoridb-parser` | lexer, AST, nGQL 기반 parser |
| `byoridb-executor` | plan, query 실행, ontology, temporal, 추천 로직 |
| `byoridb-graph` | 인증, session, query service, gRPC, HTTP, metrics |
| `byoridb-client` | Rust client와 대화형 CLI |
| `byoridb-bulkloader` | offline CSV bulk loader |

루트 `byoridb` package는 `byoridb-server`와 `byoridb-backup` binary를 빌드합니다.

## 첫 번째 쿼리

명시적인 root secret으로 서버를 시작합니다.

```bash
export BYORIDB_ROOT_PASSWORD='replace-with-a-managed-secret'
cargo run --bin byoridb-server --release
```

다른 터미널에서 접속합니다.

```bash
export BYORIDB_USER=root
export BYORIDB_PASSWORD="$BYORIDB_ROOT_PASSWORD"
cargo run -p byoridb-client --bin byoridb-cli
```

작은 그래프를 만들고 조회합니다.

```sql
CREATE SPACE example(vid_type = INT64);
USE example;

CREATE TAG person(name STRING, age INT64);
CREATE EDGE knows(since INT64);

INSERT VERTEX person(name, age) VALUES
  1:("Alice", 30),
  2:("Bob", 25);
INSERT EDGE knows(since) VALUES 1->2:(2024);

FETCH PROP ON person 1;
GO FROM 1 OVER knows YIELD dst(edge) AS friend;
MATCH (p:person) RETURN p LIMIT 10;
```

[설치](getting-started/installation.html)와
[빠른 시작](getting-started/quickstart.html)을 이어서 확인하세요.

## 중요한 경계

- native TLS는 구현되어 있지 않습니다. 신뢰할 수 있는 proxy/load balancer에서 TLS를
  종료하고 네트워크 경계에서 listener 접근을 제한해야 합니다.
- session과 활성 인증 cache는 프로세스 내부 상태입니다. 재시작 후 session은
  사라지고 replica 사이에 공유되지 않습니다.
- `AS OF` 이력은 사용자가 기록한 정점/엣지 상태를 다룹니다. 과거 inferred fact를
  재구성하지 않으며 temporal `MATCH`/`GO`는 지원하지 않습니다.
- 저장소의 Docker Compose 서비스는 서로 독립된 standalone 데이터베이스입니다.
  세 노드 클러스터가 아닙니다.

## 라이선스

ByoriDB는 Apache License 2.0으로 배포됩니다.

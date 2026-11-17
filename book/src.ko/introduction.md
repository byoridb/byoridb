# ByoriDB

[English](../introduction.html)

<p align="center">
  <img src="../assets/byoridb-icon.png" alt="ByoriDB official icon" width="256">
</p>

ByoriDB는 **AI 에이전트를 주 호출자로 설계하는** 에이전트 메모리용 Rust 그래프
데이터베이스입니다. 스키마 발견, 관계 조회, 근거 확인, 변경 준비, 호출자 권한
안에서의 적용, 결과 확인으로 이어지는 예측 가능한 실행 흐름이 제품 목표입니다.
Agent memory가 주 워크로드이며 그래프 코어는 범용 property graph로 유지됩니다.
ByoriDB는 OWL이나 RDF 저장소가 아니며, 추론은 결정적인 규칙의 제한된 집합입니다.

[Agent-first 설계](architecture/agent-first.html)는 채택한 방향과 계획 중인 계약을
설명합니다. 현재 엔진은 nGQL에서 영감을 받은 쿼리 언어, 영속 property graph,
RDFS/OWL 어휘를 쓰는 규칙 기반 전방 추론, 추천 기능, asserted vertex와 edge의 과거 시점 조회를
제공합니다. 구조화된 Agent 연산, 영속 요청 결과, revision에 묶인 변경 미리보기는
계획 단계입니다. 구현된 인터페이스는 [API 레퍼런스](reference/api.html)에 명시합니다.

현재 지원되는 배포는 **하나의 redb database를 사용하는 단일 ByoriDB 서버**입니다.
저장소에는 partition routing, Meta/Storage RPC, custom Raft component도 있지만
production-ready 다중 노드 launcher로 연결되지는 않았습니다. Cluster 배포를 계획하기
전에 [분산 시스템](architecture/distributed.html)을 확인하세요.

## 구현된 기능

- **Property graph**: space, tag, edge type, vertex, edge, secondary index.
  Edge update와 index를 사용하는 tag/edge `LOOKUP`도 포함합니다.
- **Query**: `FETCH`, `GO`, `MATCH`, `LOOKUP`, `FIND PATH`, `EXPLAIN`, `PROFILE`.
- **Ontology**: class hierarchy, semantic edge 선언, 전방 materialization,
  `owl:sameAs`, `WHY`를 통한 추론 근거, consistency check, SHACL 방식의 shape 검증.
- **추천**: 구조적 유사도, embedding 유사도, 큰 vector 집합을 위한 영속 HNSW index,
  filter, 두 신호를 혼합한 reranking.
- **시간 조회**: DML이 current view와 asserted-fact history를 유지합니다.
  `FETCH PROP`으로 epoch-millisecond 시각의 vertex 또는 edge를 `AS OF` 조회합니다.
- **인증과 권한**: 환경 변수의 root credential, 영속 non-root user, 기본 role,
  session 기반 gRPC/HTTP 접근, query 권한 검사.
- **운영**: Prometheus metrics, health/readiness endpoint, 정상 종료, snapshot
  backup CLI, Docker 파일, single-replica AKS manifest.

## 실행 구조

Standalone server는 한 프로세스에서 다음 crate를 구성합니다.

| Crate | 책임 |
|---|---|
| `byoridb-common` | 공통 graph value와 data set |
| `byoridb-kvstore` | redb 기반 current 및 history keyspace |
| `byoridb-codec` | Vertex, edge, value, row codec |
| `byoridb-storage` | Storage environment, index, RPC, partition, Raft component |
| `byoridb-meta` | Schema, host, partition, migration metadata component |
| `byoridb-parser` | Lexer, AST, nGQL 기반 parser |
| `byoridb-executor` | Plan, query 실행, ontology, temporal 및 recommendation 로직 |
| `byoridb-graph` | 인증, session, query service, gRPC, HTTP, metrics |
| `byoridb-client` | Rust client와 대화형 CLI |
| `byoridb-bulkloader` | Offline CSV bulk loader |

Root의 `byoridb` package가 `byoridb-server`와 `byoridb-backup` binary를 빌드합니다.

## 첫 query

비어 있지 않은 root secret을 명시하고 서버를 시작합니다.

```bash
export BYORIDB_ROOT_PASSWORD='replace-with-a-managed-secret'
cargo run --locked --release -p byoridb --bin byoridb-server
```

다른 terminal에서 연결합니다.

```bash
export BYORIDB_USER=root
export BYORIDB_PASSWORD='same-secret-used-to-start-the-server'
cargo run --locked -p byoridb-client --bin byoridb-cli
```

작은 graph를 생성하고 조회합니다.

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

[설치](getting-started/installation.html)와 [빠른 시작](getting-started/quickstart.html)에서
계속 진행하세요.

## 주요 제약

- Native TLS는 구현되지 않았습니다. 신뢰할 수 있는 proxy/load balancer에서 TLS를
  종료하고 network 경계에서 listener 접근을 제한하세요.
- Session과 실행 중인 authentication cache는 process-local입니다. Session은
  재시작 후 유지되지 않으며 replica 사이에 공유되지 않습니다.
- `AS OF` history는 asserted vertex/edge 상태를 다룹니다. 과거 inferred fact를
  재구성하지 않으며 temporal `MATCH`/`GO`는 구현되지 않았습니다.
- 저장소의 Docker Compose service들은 독립적인 standalone database이며 3-node
  cluster가 아닙니다.

## 라이선스

ByoriDB는 Apache License 2.0에 따라 제공됩니다.

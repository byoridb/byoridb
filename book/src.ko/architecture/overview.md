[English](../../architecture/overview.html)

# 아키텍처 개요

ByoriDB는 Rust workspace이며 기본 server는 query, metadata, storage logic을 한
process에 조합합니다. 이것이 저장소의 standalone binary와 현재 deployment
manifest가 실제로 실행하는 architecture입니다.

```text
                    +-----------------------+
                    | Rust CLI / gRPC / HTTP|
                    +-----------+-----------+
                                |
                    +-----------v-----------+
                    | shared GraphService   |
                    | auth, RBAC, sessions  |
                    +-----------+-----------+
                                |
                    +-----------v-----------+
                    | parser -> plan ->     |
                    | executor              |
                    +-----------+-----------+
                                |
                 +--------------v---------------+
                 | KVStore + indexes + metadata |
                 +--------------+---------------+
                                |
                    +-----------v-----------+
                    | redb/data.redb        |
                    | kv + history tables   |
                    +-----------------------+
```

## Standalone 요청 경로

1. gRPC 또는 HTTP adapter가 credential이나 query를 받습니다.
2. 두 protocol은 동일한 in-process `GraphService`를 사용하므로 사용자, role,
   session, active-query tracking, shutdown state를 공유합니다.
3. service가 session을 검증하고 statement를 parse한 뒤 compound와 `PROFILE`을
   포함해 authorization을 재귀적으로 확인합니다.
4. planner가 동일한 `KVStore`를 사용하는 executor를 만듭니다.
5. executor가 redb의 graph data, schema, index, ontology state, user record를
   읽거나 갱신합니다.
6. protocol adapter가 결과 `DataSet`을 protobuf 또는 JSON으로 변환합니다.

따라서 현재 runtime에서 Graph service는 **stateless가 아닙니다**. Non-root user
record는 영속 저장되고 startup 시 authentication cache에 hydrate되지만, session과
active authentication state는 process-local입니다.

## 주요 구성요소 경계

### Graph 계층

`byoridb-graph`의 책임은 다음과 같습니다.

- authentication과 built-in role 검사
- session lifecycle과 선택한 space tracking
- query parsing/authorization orchestration
- gRPC와 HTTP adapter
- active-query diagnostics, metrics, graceful drain 연동

standalone binary는 하나의 `Arc<GraphService>`를 만들어 두 network server에
전달합니다. Embedded 사용자는 service를 직접 만들 수 있습니다.

### Parser와 executor

`byoridb-parser`는 nGQL-inspired language를 AST로 변환합니다.
`byoridb-executor`는 plan을 만들고 DDL, DML, graph traversal, pattern matching,
index, ontology reasoning, recommendation, temporal read, `EXPLAIN`, `PROFILE`을
실행합니다.

executor는 `KVStore`의 logical key namespace를 사용합니다. Standalone mode에서는
별도 Storage service로 network hop을 만들지 않습니다.

### Storage와 codec

`byoridb-kvstore`는 storage abstraction과 production redb implementation을
제공합니다. `byoridb-codec`은 새 vertex/edge record를 protobuf로 encoding하며
legacy JSON record를 읽기 위한 decode fallback을 유지합니다.

redb file에는 두 개의 주요 table이 있습니다.

- `kv`: current data, schema, index, user, materialized state
- `history`: asserted vertex/edge point-in-time read를 위한 append-only version과
  tombstone

transaction과 temporal 세부사항은 [스토리지 엔진](storage.html)을 확인하세요.

### Meta, partition, RPC, Raft 구성요소

`byoridb-meta`와 `byoridb-storage`에는 Meta/Storage RPC service, partition
allocation과 migration code, partition별 custom Raft implementation이 있습니다.
distributed query executor는 Meta와 Storage client로 명시적으로 구성했을 때 일부
operation을 원격으로 route할 수 있습니다.

하지만 `byoridb-server`는 이 구성요소들을 완전히 연결하지 않습니다. Cluster peer를
설정하면 Meta gRPC server는 시작하지만 Storage/Raft peer를 bootstrap하거나 Graph
executor를 remote partition routing으로 전환하지 않습니다. 이를 지원되는
high-availability deployment로 해석해서는 안 됩니다.
[분산 시스템](distributed.html)을 확인하세요.

## 데이터와 일관성 경계

- redb write transaction은 ACID이며 write transaction은 직렬화됩니다.
- multi-row DML은 batch write를 사용합니다. Executor의 `batch_apply` 경로는
  current-view entity mutation과 대응 history version을 하나의 redb transaction으로
  commit합니다.
- 사용자에게 노출된 multi-statement transaction 문법은 없습니다. Compound
  statement는 순서대로 실행되며 뒤 clause가 실패해도 앞 clause를 rollback하지
  않습니다.
- inference는 current view에 materialize됩니다. Historical read는 asserted fact를
  보존하지만 과거 inference closure를 보존하지 않습니다.
- clean shutdown은 먼저 readiness를 중단하고 active query를 drain한 뒤 redb를
  checkpoint합니다.

## 배포 경계

저장소는 Docker Compose와 Azure AKS asset을 제공합니다. Compose는 서로 독립된
standalone database를 시작하고, AKS StatefulSet은 ReadWriteOnce volume을 사용하는
replica 하나를 선언합니다. 이 file은 저장소의 deployment intent를 설명하며 특정
live environment의 현재 상태를 증명하지 않습니다.

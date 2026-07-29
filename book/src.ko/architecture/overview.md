# 아키텍처 개요

[English](../../architecture/overview.html)

ByoriDB는 Rust workspace이며, 기본 서버는 query, metadata, storage 로직을 한
프로세스에 조합합니다. 이것이 저장소의 standalone binary와 현재 deployment
manifest가 실제로 실행하는 아키텍처입니다.

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

1. gRPC 또는 HTTP adapter가 자격 증명이나 쿼리를 받습니다.
2. 두 protocol은 동일한 in-process `GraphService`를 사용하므로 사용자, 역할,
   session, 활성 쿼리 추적, shutdown 상태를 공유합니다.
3. service가 session을 검증하고 statement를 parse한 뒤 compound와 `PROFILE`을
   포함해 권한을 재귀적으로 확인합니다.
4. planner가 동일한 `KVStore`를 사용하는 executor를 만듭니다.
5. executor가 redb의 graph data, schema, index, ontology 상태, user record를
   읽거나 갱신합니다.
6. protocol adapter가 결과 `DataSet`을 protobuf 또는 JSON으로 변환합니다.

따라서 현재 runtime에서 Graph service는 **stateless가 아닙니다**. non-root user
record는 영속 저장되고 로그인 시 인증 cache로 읽히지만, session과 활성 인증 상태는
프로세스 내부에 남습니다.

## 주요 구성요소 경계

### Graph 계층

`byoridb-graph`의 책임은 다음과 같습니다.

- 인증과 기본 역할 검사
- session 생명주기와 선택한 space 추적
- query parse/권한 orchestration
- gRPC와 HTTP adapter
- 활성 쿼리 진단, metrics, graceful drain 연동

standalone binary는 하나의 `Arc<GraphService>`를 만들어 두 network server에
전달합니다. embedded 사용자는 service를 직접 만들 수 있습니다.

### Parser와 executor

`byoridb-parser`는 nGQL 기반 언어를 AST로 변환합니다. `byoridb-executor`는 plan을
만들고 DDL, DML, graph traversal, pattern matching, index, ontology reasoning,
recommendation, temporal read, `EXPLAIN`, `PROFILE`을 실행합니다.

executor는 `KVStore`의 논리 key namespace를 사용합니다. standalone 모드에서는
별도 Storage service로 network hop을 만들지 않습니다.

### Storage와 codec

`byoridb-kvstore`는 storage abstraction과 production redb 구현을 제공합니다.
`byoridb-codec`은 새 vertex/edge record를 protobuf로 인코딩하며 과거 JSON record를
읽기 위한 fallback을 유지합니다.

redb 파일에는 두 개의 주요 table이 있습니다.

- `kv`: 현재 data, schema, index, user, materialized 상태
- `history`: asserted vertex/edge 시점 조회를 위한 append-only version과 tombstone

transaction과 temporal 세부사항은 [스토리지 엔진](storage.html)을 확인하세요.

### Meta, partition, RPC, Raft 구성요소

`byoridb-meta`와 `byoridb-storage`에는 Meta/Storage RPC service, partition allocation과
migration 코드, partition별 custom Raft 구현이 있습니다. distributed query executor는
Meta와 Storage client로 명시적으로 구성했을 때 일부 연산을 원격으로 route할 수 있습니다.

하지만 `byoridb-server`는 이 구성요소들을 완전히 연결하지 않습니다. cluster peer를
설정하면 Meta gRPC server는 시작하지만 Storage/Raft peer를 bootstrap하거나 Graph
executor를 원격 partition routing으로 전환하지 않습니다. 이를 지원되는 high-availability
배포로 해석해서는 안 됩니다. [분산 시스템](distributed.html)을 확인하세요.

## 데이터와 일관성 경계

- redb write transaction은 ACID이며 write transaction은 직렬화됩니다.
- multi-row DML은 batch write를 사용합니다. executor의 `batch_apply` 경로는 전달받은
  current-view entity 변경과 이에 대응하는 history version을 하나의 redb transaction으로
  commit합니다.
- 사용자에게 노출된 multi-statement transaction 문법은 없습니다. compound statement는
  순서대로 실행되며 뒤 clause가 실패해도 앞 clause를 rollback하지 않습니다.
- inference는 current view에 materialize됩니다. historical read는 asserted fact를
  보존하지만 과거 inference closure를 보존하지 않습니다.
- clean shutdown은 먼저 readiness를 중단하고 활성 query를 drain한 뒤 redb를 checkpoint합니다.

## 배포 경계

저장소는 Docker Compose와 Azure AKS 자산을 제공합니다. Compose는 서로 독립된
standalone database를 시작하고, AKS StatefulSet은 ReadWriteOnce volume을 사용하는
replica 하나를 선언합니다. 이 파일은 저장소의 배포 의도를 설명하며 특정 live 환경의
현재 상태를 증명하지 않습니다.

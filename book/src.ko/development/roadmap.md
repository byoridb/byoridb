# 로드맵

[English](../../development/roadmap.html)

이 페이지는 현재 source tree에서 확인할 수 있는 방향을 요약합니다. release schedule이나
compatibility 약속이 아닙니다. 자세한 engineering source of truth는 `docs/PLAN.md`입니다.
그 파일의 과거 incident 기록을 오늘 live 환경의 현재 상태로 해석하면 안 됩니다.

## 현재 main line에서 사용 가능한 기능

### Graph와 query core

- space, tag, edge type, vertex, edge, tag/edge index
- vertex `INSERT`, `UPDATE`, `DELETE`와 edge `INSERT`, `DELETE`
- `FETCH`, `GO`, `MATCH`, `LOOKUP`, path finding
- variable-length MATCH path, reverse-edge traversal, grouping과 일반 aggregate
- `EXPLAIN` access-path 보고와 runtime `PROFILE` observation
- multi-row batch DML과 scan, traversal, result materialization resource guard

### Ontology와 recommendation

- class hierarchy, disjointness, equivalent class/property
- transitive, symmetric, inverse, subproperty, domain/range, two-link property chain
- current-view forward materialization, inference provenance, `WHY`, incremental edge retraction
- 동작이 비가역임을 명시한 `owl:sameAs` canonical merge
- shape 선언, write-time 검사, consistency query
- 큰 vector 집합용 persisted HNSW index를 포함한 structural, embedding, blended recommendation

### Storage와 temporal 상태

- pure-Rust redb current-view storage와 protobuf vertex/edge payload
- 물리적으로 분리된 asserted-fact history table
- executor temporal DML 경로에서 current entity mutation과 대응 history version을 하나의
  redb transaction으로 atomic apply
- 같은 millisecond history key 충돌을 막는 monotonic transaction timestamp
- tombstone을 포함한 vertex/edge `FETCH PROP ... AS OF <epoch-ms>`
- current와 history table을 보존하는 snapshot backup/restore

### Service와 운영

- 하나의 service instance를 공유하는 인증된 gRPC/HTTP query service
- durable non-root user, 기본 role, recursive statement authorization, security-state 변경
  후 session invalidation, admin-only query diagnostics
- Prometheus query metrics, health/readiness endpoint, graceful drain
- interactive Rust CLI, offline CSV bulk loader, Docker 자산, single-replica Azure AKS 배포 정의

## 현재 제품 경계

다음 제약은 완료된 기능이 아니라 현재 상태의 일부입니다.

- **Multi-node operation:** partition, RPC, Meta, migration, custom Raft 구성요소가 있지만
  launcher는 완전한 Storage/Raft cluster를 연결하거나 일반 Graph query를 그 경로로
  route하지 않습니다.
- **Temporal semantics:** valid time을 사용자가 지정할 수 없고 하나의 epoch-ms 값이 valid와
  transaction time에 함께 사용됩니다. temporal `MATCH`/`GO`, interval, historical inferred
  fact가 없습니다.
- **Session:** session과 active-auth 상태는 process 내부에 있으며 restart 시 사라지고
  replica 사이에 공유되지 않습니다.
- **Transport security:** native TLS가 없습니다. 신뢰할 수 있는 TLS termination,
  network restriction, 외부 traffic control이 필요합니다.
- **Transaction:** redb 연산은 transactional이지만 query language에 일반 multi-statement
  transaction이나 compound rollback이 없습니다.
- **Edge update:** parser는 `UPDATE EDGE`를 받아들이지만 현재 plan/executor 경로는 vertex
  update만 구현합니다.
- **API maturity:** gRPC structured response의 complex value는 JSON fallback을 사용하고,
  과거 JSON byte field는 compatibility를 위해 남아 있습니다.
- **운영 packaging:** 지원되는 Kubernetes operator, Helm chart, 자동 multi-node upgrade
  절차가 없습니다.

## 진행 방향

### Distributed runtime 완성

가장 큰 architecture gap은 이미 존재하는 구성요소를 연결하고 검증하는 것입니다.

- Storage RPC와 partition별 Raft 시작
- peer discovery, bootstrap, membership change, leader routing
- query type 전체의 distributed Graph execution parity
- 실제 multi-process 배포에서 replication/recovery/failover test
- 여러 Graph replica를 위한 session/authorization 설계
- upgrade, snapshot, migration, observability runbook

공유 데이터에 replica 2개 이상을 권장하려면 이 작업이 먼저 닫혀야 합니다.

### Bitemporal query 확장

다음 temporal 증분 후보에는 명시적 valid-time interval, 독립 transaction-time 선택,
interval query, temporal graph traversal/pattern matching, inferred history 정책이 있습니다.
어떤 확장도 current-view 성능과 backup compatibility를 보존해야 합니다.

### 실행 scalability 개선

parallel execution과 더 나은 cost-based planning은 측정 기반 후속 작업입니다. memory를
제한하고, 의도하지 않은 full scan을 피하고, 대규모 aggregation 경로를 개선하며, 고정된
marketing QPS 대신 재현 가능한 benchmark를 공개하는 것이 우선입니다.

### 운영과 security 강화

native 또는 명확히 문서화된 TLS integration, external rate limiting, 더 완전한
space-scoped 권한 관리, cluster-wide session revoke, backup automation, restore drill,
그리고 선언된 storage/session/partition metric을 실제 runtime 상태로 공급하는 작업이
남아 있습니다.

### Client와 wire format 성숙

현재 구현된 client 표면은 Rust client입니다. complex value의 풍부한 first-class protobuf
표현과 다른 언어 client는 임의 wire 변경이 아니라 명시적인 compatibility policy를
따라야 합니다.

## 기여 방법

issue 또는 `docs/PLAN.md`의 구체적 항목을 선택하고 변경 범위를 작게 유지하며
[기여 가이드](contributing.html)를 따르세요. distributed, Raft, temporal, authentication,
storage 변경에는 전문 regression test를 포함하고 새 기능만큼 남은 경계도 정확하게
문서화해야 합니다.

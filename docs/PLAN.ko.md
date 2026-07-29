# ByoriDB 계획

> [English](PLAN.md) | **한국어**
>
> 최종 코드 검증: 2026-07-29, `76e7a79`와 이번 변경의 인증·인가 강화 패치 기준.
> 이 문서는 코드와 저장소 상태를 기록하며, 특정 Kubernetes 클러스터의 실시간
> 상태를 기록하지 않습니다.

이 문서는 프로젝트 계획의 단일 진실원입니다. 과거 여러 로드맵과 개선 문서에
흩어져 있던 상태를 통합합니다. 이 문서가 늦게 갱신된 경우 구현과 테스트를 우선합니다.

## 상태 표기

- **지원**: `main` 또는 이번 변경에 구현되어 있고 자동화 테스트가 있음.
- **부분 지원**: 유용한 구성요소는 있지만 제품의 end-to-end 표면이 미완성이거나
  프로덕션에 적합하지 않음.
- **계획**: 아직 지원되는 구현이 없음.
- **제약**: 호출자가 설계 단계에서 고려해야 하는 의도적 또는 알려진 한계.

## 제품 방향

ByoriDB는 Rust로 작성한 시맨틱 그래프 데이터베이스 코어입니다. labelled-property
graph 저장·쿼리 모델을 유지하면서 온톨로지 추론, 시간 이력, provenance, 설명 기능을
데이터베이스 primitive로 추가합니다.

코어는 운영 모델링 애플리케이션 플랫폼을 지향하지 않습니다.

| ByoriDB 코어 | 애플리케이션 또는 Studio 레이어 |
|---|---|
| 클래스, 그래프 스키마, 추론 규칙 | 데이터 소스 매핑과 ETL |
| Provenance와 `WHY` 설명 | Object/action/function UX |
| 제약 및 shape 평가 | Workflow와 write-back 오케스트레이션 |
| 현재 및 과거의 asserted fact | 비즈니스 감사와 승인 흐름 |
| 그래프 순회와 추천 | 도메인별 사용자 경험 |

단기 목표는 정확하고 안전한 단일 노드 배포입니다. 실제 다중 노드 launcher는 아직
로드맵 항목입니다.

## 검증된 기준선

| 영역 | 현재 상태 |
|---|---|
| 툴체인 | Rust 1.90, edition 2021, gRPC 코드 생성에 protobuf compiler 필요 |
| 패키지 메타데이터 | 워크스페이스 루트는 `0.3.3`; 유지되는 semver 릴리스 대신 배포물을 커밋 SHA로 식별 |
| 서버 | 하나의 `byoridb-server` 프로세스가 공유 `GraphService`와 redb 저장소 위에서 gRPC와 HTTP 제공 |
| 기본 listener | gRPC `0.0.0.0:9669`, HTTP `0.0.0.0:19669`; 둘 다 native TLS 미지원 |
| 저장소 | redb current view와 같은 데이터베이스 안의 별도 history table |
| 내구성 | 기본은 Immediate/fsync; relaxed 내구성은 bulk load 전용의 명시적 trade-off |
| 인증 | Argon2 비밀번호 해시, 무작위 양수 세션 ID, 기본 세션 TTL 24시간 |
| 테스트 게이트 | `cargo test --workspace --all-features -- --test-threads=1` |
| 정적 게이트 | `cargo fmt --all -- --check`, 워크스페이스 Clippy `-D warnings` |

위 전체 테스트·포맷·Clippy 게이트는 2026-07-29에 통과했습니다. 회귀 테스트가 추가될
때마다 달라지는 전체 테스트 개수는 문서에 고정하지 않습니다.

## 지원 기능

### 그래프 및 쿼리 실행

- Space, tag, edge, class, index, shape, 사용자 관리 문장.
- Vertex insert/update/delete/fetch/lookup과 edge insert/delete/fetch/traversal.
- `MATCH`, `GO`, `FIND PATH`, compound statement, `EXPLAIN`, `PROFILE`.
- Hash/modulo/range 파티션 메타데이터와 분산 쿼리 구성요소.
- Tag/edge index, reverse-edge index, tag-to-VID index, embedding 추천용 영속 HNSW index.
- 쿼리 scan, traversal, path 개수, 추정 결과 메모리 제한.

파서는 선택적인 영역에서 nGQL과 호환되며 NebulaGraph nGQL 전체 구현은 아닙니다.
문서의 예제는 parser 또는 end-to-end 테스트로 뒷받침되어야 합니다.

### 시맨틱 그래프 레이어

- 다중 부모, disjointness, equivalent class를 포함한 클래스 계층.
- Subproperty, inverse, symmetric, transitive, equivalent-property, domain,
  range, 2-link property-chain 의미론.
- Current view에 대한 write-time forward materialization.
- 지원되는 inferred fact의 provenance, `WHY` 설명, DRed 방식 retraction.
- 예약된 `sameAs` edge를 이용한 `owl:sameAs` 방식 canonical merge.
- SHACL에서 영감을 받은 required, datatype, predicate 제약.
- 구조적 유사도, embedding 유사도, 영속 HNSW 검색, blended semantic/graph 추천.

현재 `sameAs` 병합은 의도적으로 되돌릴 수 없습니다. 병합된 vertex, 예약 edge type,
`sameAs` assertion 삭제는 거부됩니다.

### 시간 이력

Asserted vertex/edge 변경은 하나의 `batch_apply` 트랜잭션에서 current view를 갱신하고
history를 append합니다. 같은 millisecond의 key 충돌을 막기 위해 transaction timestamp를
단조 증가로 할당합니다. redb와 memory backend는 seek 기반 시점 해석을 제공합니다.

현재 공개 쿼리 표면은 다음을 지원합니다.

```ngql
FETCH PROP ON person 42 AS OF 1780000000000;
FETCH PROP ON follows 1->2 AS OF 1780000000000;
FETCH PROP ON * 1->2 AS OF 1780000000000;
```

이 표면에서는 하나의 `AS OF` 값이 valid time과 transaction time 양쪽에 적용됩니다.
쓰기 시 두 축 모두 엔진이 할당한 현재 시간을 사용합니다.

### 보안 경계

현재 단일 프로세스 보안 모델은 다음을 포함합니다.

- Standalone 서버의 비어 있지 않은 `BYORIDB_ROOT_PASSWORD` 필수 조건.
- 계정 열거 신호를 줄이기 위한 공통 인증 오류와 알 수 없는 사용자의 dummy 비밀번호 검증.
- 과거에 빈 비밀번호 해시로 저장된 레코드까지 포함한 blank password 거부.
- Compound statement와 실행되는 `PROFILE` 문장의 재귀 인가.
- 사용자·role 변경, balance, 민감한 session/user 조회의 GOD/ADMIN 제한.
- 로컬 비밀번호·role·활성 상태·사용자 변경 후 세션 무효화.
- 공유 HTTP/gRPC 인증 상태와 race-safe durable-user cache 동기화.
- Diagnostics, 로그, invalid-session 응답의 비밀번호 쿼리와 bearer session ID redaction.
- Active-query diagnostics endpoint의 관리자 인증.

지원되는 보안 신고 채널과 운영자가 적용해야 할 배포 통제는
[SECURITY.ko.md](../SECURITY.ko.md)를 참고하세요.

## 알려진 제약

### 보안 및 tenancy

1. **Native 전송 암호화가 없습니다.** 신뢰하지 않는 네트워크에서는 비밀번호와 bearer
   세션이 탈취될 수 있습니다. Native TLS가 추가되기 전까지 사설망과 신뢰할 수 있는
   TLS/mTLS proxy를 사용해야 합니다.
2. **실효성 있는 로그인 rate limit이 없습니다.** 실패 카운터는 존재하지만 올바른
   비밀번호는 차단하지 않으며, 병렬 Argon2 검증이 async worker와 CPU를 소모할 수
   있습니다. 노출된 배포에는 외부 rate limit을 적용하세요.
3. **Space-scoped grant가 없습니다.** 기본 role 권한은 `space="*"`를 사용하고,
   공개 `GRANT`/`REVOKE` 문법은 space ACL이 아니라 role을 할당합니다.
4. **넓은 `Write` 의미론을 사용합니다.** `INSERT`, `UPDATE`, `DELETE`는 현재
   `Write`에, `ALTER`는 `Create`에 매핑됩니다. 일반 문장에서 `Delete`와 `Alter`
   permission variant를 별도 집행하지 않습니다.
5. **Auth 상태가 프로세스 로컬입니다.** 사용자·세션 cache가 서버 프로세스 간
   조정되지 않아 즉시 cross-node revoke를 보장하지 않습니다.
6. **공개 운영 endpoint가 있습니다.** `/metrics`와 `/api/v1/metrics`는 인증을
   요구하지 않습니다. HTTP sign-out route는 bearer 성격의 session ID를 URL path에
   담으므로 access log에서 redaction해야 합니다.

### 분산

Raft, snapshot, membership type, partition routing, storage RPC, failure detection,
분산 쿼리 helper가 있고 unit/integration test가 있습니다. Peer 설정 시 서버가 Meta
gRPC listener를 시작할 수 있습니다.

하지만 다음 이유로 **부분 지원**이며 다중 노드 배포를 지원한다고 볼 수 없습니다.

- Storage/Raft peer bootstrap이 end-to-end로 연결되지 않음.
- Docker Compose 서비스에 cluster 설정이 없어 각각 독립 단일 노드로 동작.
- AKS manifest가 검증된 다중 노드 topology를 구성하지 않음.
- 프로덕션 multi-node failover 또는 partition 이동 E2E gate가 없음.
- Session과 authorization 상태가 cluster 전체에 공유되지 않음.

### Temporal

- `VALID FROM`, `VALID TO`, `BETWEEN`, temporal `MATCH`, temporal `GO` 미구현.
- 과거 inferred fact를 재구성하지 않으며 inference는 current view만 읽고 씀.
- Retention, garbage collection, 사용자용 history-list API가 없음.
- Millisecond당 두 번 이상 쓰면 단조 transaction timestamp가 잠시 wall clock보다
  앞설 수 있음.

### 쿼리 및 데이터 모델

- Space DDL은 `FIXED_STRING`을 받지만 DML 실행은 여전히 integer VID를 요구합니다.
  전체 타입 구현 전까지 `INT64`를 사용하세요.
- `SHOW USER`는 현재 root 전용 placeholder를 반환합니다. `SHOW SESSIONS`는 bearer
  session ID 없이 active user와 선택된 space를 나열합니다. Public parser는
  `SHOW USERS`와 `SHOW ROLES`를 모두 허용하지 않습니다.
- `UPDATE EDGE`는 parser에만 있고 동작하는 실행 경로가 없습니다. Edge `LOOKUP`은
  거부되며 range predicate는 index를 사용하지 않습니다.
- Geography encoding은 있지만 WKT/WKB decoding은 미구현입니다.
- 많은 쿼리 경로가 physical operator pull stream 대신 중간 row를 materialize합니다.
- Reverse-edge index 도입 전 생성된 데이터는 reverse-edge 데이터를 재구축해야 합니다.

### 운영

- 저장소에 배포 manifest와 workflow가 있지만 이 문서는 live cluster의 존재나
  정상 상태를 주장하지 않습니다.
- Prometheus 출력과 구조화 로그는 있지만 유지되는 Grafana dashboard, alert rule,
  log shipping stack은 없습니다.
- Backup은 전체 redb snapshot입니다. Incremental backup, WAL archive, object-store
  upload, point-in-time restore workflow는 없습니다.

## 우선순위 로드맵

### P0 — 프로덕션 보안 경계

| ID | 작업 | 종료 조건 |
|---|---|---|
| SEC-1 | Native TLS/mTLS 또는 문서화·검증된 TLS proxy profile | 비신뢰 네트워크에 credential 평문 노출 없음, rotation 문서·테스트 완료 |
| SEC-2 | 인증 부하 제어 | Source/account별 throttling, Argon2 동시성 제한, blocking 작업의 async worker 분리 |
| SEC-3 | Space-scoped authorization | Grant 문법, durable ACL, 모든 문장과 compound/profile nesting의 집행 테스트 |
| SEC-4 | Cluster-wide identity/session 전략 | 지원되는 서버 프로세스 전체에서 revoke와 권한 변경 일관성 보장 |
| SEC-5 | 운영 endpoint 강화 | Metrics 접근 정책과 bearer-free sign-out API를 명시하고 테스트 |

### P0 — 정확성 및 운영성

| ID | 작업 | 종료 조건 |
|---|---|---|
| COR-1 | `FIXED_STRING` VID 불일치 해소 | Parser/plan/codec/key 전체에 string VID 구현 또는 DDL에서 명확히 거부 |
| COR-2 | Identity metadata 표면 완성 | 지원되는 user·role·session 목록 문법이 secret 없이 durable·인가된 상태를 반환 |
| COR-3 | Release/archive 완결성 | Binary archive에 `LICENSE`/`NOTICES` 포함, 지원 platform 명시 |
| OPS-1 | 복구 검증 | Current/history table을 모두 포함한 자동 restore test와 복구 시간 기록 |

### P1 — 시맨틱 확장

| ID | 작업 | 참고 |
|---|---|---|
| SEM-1 | Functional/inverse-functional property | Canonical merge와 안전하게 통합해야 함 |
| SEM-2 | 일반 property chain | 현재 2-link 구현을 bounded planning과 provenance로 확장 |
| SEM-3 | 완전한 설명 | Inferred vertex type과 domain/range 경로를 일관되게 포함 |
| SEM-4 | 강화된 shape | Edge cardinality와 선택적 closed-world constraint |
| SEM-5 | 분산 materialization | Multi-node runtime 지원 후 진행 |

### P1 — Temporal 쿼리 표면

| ID | 작업 | 참고 |
|---|---|---|
| TMP-1 | 명시적 valid-time 쓰기 | 독립 transaction time을 가진 `VALID FROM`/`VALID TO` |
| TMP-2 | Temporal traversal 및 pattern | History 위의 `MATCH`, `GO`, range 의미론 |
| TMP-3 | History inspection | Version-list API와 `BETWEEN` 표면 |
| TMP-4 | Lifecycle policy | Retention, compaction/GC, backup policy, 운영 metric |
| TMP-5 | Derived history 연구 | 과거 inferred fact의 재현 가능성과 방법 정의 |

### P1 — 지원 가능한 분산 실행

| ID | 작업 | 종료 조건 |
|---|---|---|
| DIST-1 | Storage/Raft bootstrap | 수동 state 수정 없이 peer discovery, group 형성, membership 복구 |
| DIST-2 | 배포 wiring | Compose/Kubernetes가 독립 서버가 아닌 하나의 cluster를 구성 |
| DIST-3 | Failure E2E | Multi-node write/read, leader loss, snapshot restore, membership change, rolling restart 테스트 |
| DIST-4 | 분산 쿼리 완성 | Partition pruning, aggregation, join, sort, bounded partial-result 의미론 |

### P2 — 측정 기반 성능 및 실행 아키텍처

- `LOOKUP` indexed range scan 추가([issue #1](https://github.com/byoridb/byoridb/issues/1)).
- 대규모 multi-ID fetch와 projection 강화
  ([issue #10](https://github.com/byoridb/byoridb/issues/10)).
- 재현 가능한 workload에서 단일 stream scan이 병목임이 확인된 뒤에 parallel range
  scan과 partial aggregation 추가.
- 변수 의미를 바꾸지 않는 `MATCH` selectivity reorder.
- [VOLCANO_MIGRATION_PLAN.ko.md](VOLCANO_MIGRATION_PLAN.ko.md)의 pull-based
  physical operator 방향으로 점진 이동.
- Profiling이 wire/decode 비용을 확인한 경우 lightweight edge destination 및
  weighted-property decoding 추가.

### P2 — API 및 관측성

- JSON fallback complex value를 first-class protobuf message로 승격.
- Dashboard, alert rule, 지원되는 log-shipping 예제 공개.
- HTTP decimal-string session ID와 클라이언트 언어 정밀도 compatibility test 추가.
- Semver 호환성 보장 전 안정적인 release/versioning 정책 정의.

## 회귀 민감 영역

### 쿼리 정확성 H 시리즈

H-1부터 H-6까지는 수정됐으며 계속 회귀 테스트해야 합니다.

| ID | 회귀 항목 |
|---|---|
| H-1 | `SHOW SPACES`의 서로 다르고 영속적인 space ID |
| H-2 | Space 간 tag/edge metadata leak 방지 |
| H-3 | `GO`가 VID 0이 아닌 실제 destination 반환 |
| H-4 | `LOOKUP` protobuf vertex decode 시 VID 0 fallback 방지 |
| H-5 | Edge `FETCH PROP`가 vertex가 아닌 edge 해석 |
| H-6 | Comma-separated `MATCH`가 모든 clause와 binding 보존 |

관련 parser/schema/key/plan/executor 변경 후 `test_h1_*`부터 `test_h5_*`,
`match_impl::h6_multipattern_tests`를 실행합니다.

### Temporal 경로

KV history table, DML, temporal executor, parser, plan 변경 시 history와 current-view
동작을 모두 검증해야 합니다.

```bash
cargo test -p byoridb-kvstore --test temporal -- --test-threads=1
cargo test -p byoridb-parser fetch_as_of -- --test-threads=1
cargo test -p byoridb-executor fetch_as_of -- --test-threads=1
cargo test --workspace --all-features -- --test-threads=1
```

### Raft 경로

`byoridb-storage/src/raft/`는 외부 compatibility certification이 없는 커스텀
구현입니다. 변경 시 전체 워크스페이스 게이트와 분산 E2E 테스트를 실행해야 합니다.

### 인증·인가 경로

Graph auth, session, HTTP/gRPC wiring, user execution 변경 시 다음을 검증합니다.

- 직접·compound·profiled mutation에 대한 guest 거부.
- 관리자 전용 user/session operation.
- Durable user hydration과 root record 격리.
- Role/password/user 변경 후 로컬 session 무효화.
- Password와 session ID redaction.
- Invalid/blank credential 동작.
- Authentication, revoke, diagnostics, sign-out 동시성.

집중 통합 테스트는 `tests/security_authz_test.rs`입니다.

## 완료 마일스톤

| 날짜 | 마일스톤 |
|---|---|
| 2026-06 | Reverse index, variable-length path, class hierarchy, semantic edge rule, materialization, consistency, `is_a`, canonical `sameAs`, provenance/`WHY`, DRed retraction, 2-link property chain |
| 2026-06 | Structural, embedding, HNSW, blended recommendation |
| 2026-07-01 | Shape validation, equivalent class/property 지원 |
| 2026-07-10 | Asserted vertex/edge temporal history와 vertex `AS OF` |
| 2026-07-13 | Atomic current/history write, monotonic transaction time, seek 기반 resolution, temporal E2E |
| 2026-07-14 | 음수 integer VID를 포함한 edge 및 wildcard-edge `AS OF` 읽기 |
| 2026-07-29 변경 | Recursive RBAC, durable auth/cache reconciliation, session revoke, credential redaction, 공유 HTTP/gRPC auth 상태, root secret 강화 |

## 의사결정 원칙

1. 기능 표면 추가 전 입증된 보안 또는 정확성 결함을 먼저 수정합니다.
2. 배포 wiring과 failure E2E가 없으면 분산을 지원한다고 표현하지 않습니다.
3. Temporal history 확장 시 current-view fast path를 보존합니다.
4. Semantic rule은 provenance와 삭제/retraction 동작을 먼저 정의한 뒤 추가합니다.
5. 대규모 실행 엔진 최적화 전 측정된 workload를 요구합니다.
6. Application workflow, data-source integration, write-back logic을 DB 코어에 넣지 않습니다.
7. 유지되는 release line이 생길 때까지 배포물은 commit SHA로 식별하고 문서에 임의
   version을 만들지 않습니다.

## 표준 검증

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features -- --test-threads=1
cargo build --workspace --release
```

문서만 변경한 경우에도 영어·한국어 mdBook을 모두 빌드하고 두 source tree의 page
path가 같은지 검증해야 합니다.

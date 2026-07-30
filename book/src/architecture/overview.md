# 아키텍처 개요

ByoriDB 코드는 세 개의 주요 컴포넌트로 스토리지-컴퓨팅 분리를 지향합니다. 다만 아래
분산 토폴로지는 목표 구조입니다. 현재 지원되는 standalone 경로는 한 프로세스에서
Storage lifecycle/redb를 열고 Graph HTTP/gRPC가 같은 KVStore를 직접 사용하는 단일
노드이며, Meta gRPC는 cluster peers가 설정된 경우에만 시작합니다.

## 시스템 아키텍처

```
┌─────────────────────────────────────────────────────────────┐
│                         Clients                              │
│              (CLI, SDKs, HTTP, gRPC)                        │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                      Graph Service                           │
│  ┌─────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐    │
│  │ Parser  │→ │ Planner  │→ │ Executor │→ │ Results  │    │
│  └─────────┘  └──────────┘  └──────────┘  └──────────┘    │
└─────────────────────────────────────────────────────────────┘
           │                              │
           ▼                              ▼
┌─────────────────────┐      ┌─────────────────────────────────┐
│    Meta Service     │      │       Storage Service           │
│  ┌───────────────┐  │      │  ┌───────────┐  ┌───────────┐  │
│  │ Schema Cache  │  │      │  │  Part 1   │  │  Part 2   │  │
│  │ Space Config  │  │      │  │  (Raft)   │  │  (Raft)   │  │
│  │ Partition Map │  │      │  └───────────┘  └───────────┘  │
│  └───────────────┘  │      └─────────────────────────────────┘
└─────────────────────┘                    │
           │                               ▼
           ▼                      ┌─────────────────┐
    ┌──────────────┐              │    KVStore      │
    │   KVStore    │              │    (redb)       │
    │    (redb)    │              └─────────────────┘
    └──────────────┘
```

## 구성 요소

### Graph Service

쿼리 실행과 인증·세션을 담당하는 API 계층입니다.

- **쿼리 파싱**: `byoridb-parser`를 사용해 nGQL → AST 변환
- **쿼리 계획**: AST → 실행 계획(Execution Plan)
- **쿼리 실행**: Meta 및 Storage 서비스와 협력
- **결과 집계**: 여러 파티션의 결과를 결합

주요 특성:
- 쿼리 계획/실행 상태는 요청 단위지만 인증 세션은 프로세스에 유지
- gRPC 및 HTTP 엔드포인트 제공
- 하위 서비스에 대한 커넥션 풀링

### Meta Service

모든 메타데이터를 관리합니다.

- **Spaces**: 논리적 데이터베이스
- **Schemas**: Tag, Edge, Index
- **Partitions**: 데이터 분산 매핑
- **Schema Versions**: 온라인 스키마 변경용

주요 특성:
- 단일 리더 (Raft를 통해 복제 가능)
- TTL을 갖는 인메모리 스키마 캐시
- KVStore에 영속 저장

### Storage Service

그래프 데이터를 저장하고 조회합니다.

- **Vertices**: VID를 키로 하는 Tag 데이터
- **Edges**: (src, edge_type, rank, dst)를 키로 하는 Edge 데이터
- **Partitioning**: VID 기반 consistent hashing
- **Replication**: Raft 합의를 통한 다중 복제

주요 특성:
- VID 기준으로 수평 분할
- 각 파티션은 자체 Raft 그룹을 가짐
- predicate pushdown 지원

### KVStore

기반 스토리지 엔진입니다.

- **redb**: 순수 Rust로 구현된 임베디드 B-tree 스토리지
- **Raft**: 분산 합의 프로토콜
- **Snapshots**: current view와 bitemporal history를 함께 보존하는 백업
- **Space reclamation**: redb free page 재사용(LSM식 background compaction 없음)

## 데이터 흐름

현재 standalone의 실제 경로는
`Client → Graph HTTP/gRPC → in-process Planner/Executor → embedded KVStore(redb)`입니다.
아래 흐름은 Storage/Raft bootstrap과 배포 wiring이 완료된 뒤의 분산 목표 경로입니다.

### 분산 목표 쓰기 경로(Write Path)

```
1. Client → Graph Service (INSERT VERTEX)
2. Graph Service → Meta Service (get partition info)
3. Graph Service → Storage Service (write to leader)
4. Storage Leader → Raft Log (replicate)
5. Storage Leader → redb (apply)
6. Ack back to client
```

### 분산 목표 읽기 경로(Read Path)

```
1. Client → Graph Service (FETCH PROP)
2. Graph Service → Meta Service (get schema + partition)
3. Graph Service → Storage Service (read from any replica)
4. Storage → redb → Return data
5. Graph Service → Apply schema version transformation
6. Return to client
```

## 크레이트 의존 관계

```
byoridb-client
    └── byoridb
            ├── byoridb-executor
            │       └── byoridb-parser
            ├── byoridb-meta
            │       └── byoridb-kvstore
            └── byoridb-storage
                    ├── byoridb-codec
                    └── byoridb-kvstore
                            └── byoridb-common
```

# 분산 시스템

[English](../../architecture/distributed.html)

> **상태: 구성요소 구현 단계이며 지원되는 cluster 배포가 아닙니다.**
> 미완성 distributed 경로 자체를 개발하고 검증하는 경우가 아니라면 ByoriDB를 단일
> server로 실행하세요.

ByoriDB에는 상당한 distributed-system building block이 있지만, 이러한 module의 존재가
현재 `byoridb-server` binary가 replicated cluster를 구성한다는 뜻은 아닙니다.

## 저장소에 있는 구성요소

### Partition routing

space는 `partition_num`과 `replica_factor` metadata를 가질 수 있습니다. 공통 hash
함수는 VID를 1부터 시작하는 partition으로 매핑하고, distributed executor는 vertex와
edge 요청을 partition별로 묶을 수 있습니다. edge 요청은 source VID로 partition됩니다.

Meta 구성요소는 space, host, partition allocation record를 관리합니다. distributed
query executor는 `MetaClient`를 조회하고 Storage host를 선택해 병렬 RPC를 보내며 일부
`FETCH`, edge, scan, index 연산을 집계할 수 있습니다.

### Storage RPC

`byoridb-storage`는 vertex/edge 접근, scan, index, partition migration, Raft transport를
위한 protobuf service를 정의합니다. `byoridb-meta`에도 migration과 rebalance helper가
있습니다.

이 service와 client는 library 구성요소입니다. 기본 launcher는 완전한 remote Storage
service 집합을 시작하고 연결하지 않습니다.

### Custom Raft

`byoridb-storage/src/raft/`는 다음 기능을 가진 custom Raft state machine과 transport를
구현합니다.

- follower, candidate, leader 상태
- request-vote와 append-entries 처리
- 영속 term/vote/log 상태
- chunked snapshot 설치
- `(space_id, part_id)`별 group 관리
- configuration-change command와 gRPC network driver

코드에는 unit/component test가 있지만 production consensus 구현으로 외부 검증되지
않았습니다. 현재 server 배포의 data replication이나 failover를 보장한다고 표현하면
안 됩니다.

## 현재 launcher 동작

`byoridb-server`는 다음 cluster 설정을 읽습니다.

```text
BYORIDB__CLUSTER__NODE_ID
BYORIDB__CLUSTER__PEERS
BYORIDB__CLUSTER__ADVERTISE_ADDR
BYORIDB__CLUSTER__BOOTSTRAP
BYORIDB__CLUSTER__META_ADDR
```

`BYORIDB__CLUSTER__PEERS`가 비어 있으면 일반 standalone 경로로 실행합니다. 값이 있으면
launcher가 Meta gRPC server를 추가로 시작합니다. 현재는 다음 작업을 하지 않습니다.

- peer 목록에서 Storage/Raft peer bootstrap
- 모든 partition에 필요한 Storage query/Raft RPC topology 시작
- remote Meta/Storage client를 포함한 Graph execution context 구성
- 일반 Graph query를 distributed executor로 route
- 완전한 membership/bootstrap lifecycle 구현
- 인증 session 공유 또는 재시작 영속화

`BYORIDB__CLUSTER__BOOTSTRAP`은 parse되지만 아직 완전한 bootstrap sequence에
연결되지 않았습니다.

## 배포 파일은 standalone 구성

저장소의 배포 자산은 cluster를 구성하지 않습니다.

- `docker-compose.yml`은 각자 volume을 사용하고 cluster 환경변수가 없는 세 개의 독립
  `byoridb-server` process를 시작합니다. 쓰기는 서로 복제되지 않습니다.
- `deploy/azure/k8s/03-statefulset.yaml`은 replica 하나와 ReadWriteOnce PVC 하나를
  선언합니다.

distributed storage와 session routing을 완성하지 않은 채 replica 수를 바꾸면 서로
격리된 database와 일관되지 않은 client 동작이 발생할 수 있습니다. 독립 replica를
데이터를 공유하는 것처럼 한 load balancer 뒤에 배치하지 마세요.

repository manifest는 desired configuration만 설명합니다. 이 페이지는 실제 Kubernetes나
Azure 환경의 현재 상태를 주장하지 않습니다.

## Session과 권한 제약

non-root user record는 redb에 저장되고 in-process auth cache로 읽힙니다. session ID,
선택한 space, role snapshot, active-query 상태는 process-local입니다. 따라서 미래의
multi-Graph 배포에는 명확한 session affinity 또는 shared-session 설계와 cluster-wide
revocation 동작도 필요합니다.

## 지원되는 cluster에 필요한 작업

production-ready distributed 모드에는 최소한 다음 작업이 필요합니다.

1. Storage RPC, Raft driver, peer discovery, group bootstrap launcher wiring
2. 모든 지원 query 경로에서 Meta/Storage client에 연결된 Graph context와 명시적인
   local/distributed 동작 일치
3. membership change, leader redirect, recovery, snapshot, data migration 검증
4. Graph replica 사이의 인증/session 동작 정의
5. component routing만이 아니라 replication과 failover를 검증하는 Docker/Kubernetes
   topology와 multi-process end-to-end test
6. 운영 runbook, upgrade compatibility, observability, fault injection test

이 gate가 닫힐 때까지 `partition_num`과 `replica_factor`는 schema/component metadata로
취급해야 하며 standalone server가 물리 replica를 만들었다는 근거로 사용하면 안 됩니다.

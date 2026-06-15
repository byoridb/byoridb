# 분산 시스템

ByoriDB는 고가용성을 갖춘 분산 배포를 위해 설계되었습니다.

## Raft 합의

각 파티션은 다음을 위해 Raft 프로토콜을 사용합니다.

- **Leader Election**: 리더 장애 시 자동 페일오버(failover)
- **Log Replication**: 모든 쓰기는 리더를 거쳐 팔로워로 복제됨
- **Consistency**: 각 파티션 내에서 강한 일관성(strong consistency) 보장

### Raft 상태

```
┌─────────┐  timeout   ┌───────────┐  majority vote  ┌────────┐
│Follower │──────────→ │ Candidate │───────────────→ │ Leader │
└─────────┘            └───────────┘                 └────────┘
     ↑                       │                            │
     │        split vote     │                            │
     └───────────────────────┘                            │
     │                                     heartbeat      │
     └────────────────────────────────────────────────────┘
```

### 설정

```toml
[raft]
election_timeout_ms = 1000     # Time before new election
heartbeat_interval_ms = 100    # Leader heartbeat frequency
snapshot_interval = 10000      # Entries before snapshot
max_log_entries = 100000       # Max log entries to keep
```

## 파티셔닝

데이터는 VID 기반 해싱을 사용해 여러 파티션에 분산됩니다.

```
partition_id = hash(vid) % num_partitions
```

### 파티션 설정

```sql
CREATE SPACE my_space(
    vid_type = INT64,
    partition_num = 10,
    replica_factor = 3
);
```

| 파라미터 | 설명 | 권장값 |
|-----------|-------------|----------------|
| `partition_num` | 파티션 개수 | 머신당 10-100 |
| `replica_factor` | 각 파티션의 복제본 수 | 프로덕션에서는 3 |

### 파티션 분포

```
┌──────────────────────────────────────────────────────────┐
│                    Storage Cluster                        │
│                                                          │
│  Node 1          Node 2          Node 3                  │
│  ┌────┐          ┌────┐          ┌────┐                 │
│  │P1-L│          │P1-F│          │P1-F│   Partition 1   │
│  ├────┤          ├────┤          ├────┤                 │
│  │P2-F│          │P2-L│          │P2-F│   Partition 2   │
│  ├────┤          ├────┤          ├────┤                 │
│  │P3-F│          │P3-F│          │P3-L│   Partition 3   │
│  └────┘          └────┘          └────┘                 │
│                                                          │
│  L = Leader, F = Follower                               │
└──────────────────────────────────────────────────────────┘
```

## 복제(Replication)

### 쓰기 복제

1. 클라이언트가 Graph Service로 쓰기 요청을 보냄
2. Graph Service가 파티션 리더로 라우팅
3. 리더가 Raft log에 추가(append)
4. 리더가 팔로워로 복제
5. 과반수(majority)가 확인하면 commit
6. redb에 적용하고 응답

### 읽기 옵션

| 모드 | 일관성 | 성능 |
|------|-------------|-------------|
| Leader | 강함(Strong) | 낮음 |
| Follower | 최종적(Eventual) | 높음 |

## 장애 처리

### 리더 장애

1. 팔로워가 heartbeat 누락을 감지
2. election timeout이 새 선거를 트리거
3. 새 리더 선출 (과반수 필요)
4. 새 리더와 함께 서비스 재개

**복구 시간:** election timeout의 약 2-3배

### 팔로워 장애

1. 리더가 팔로워의 다운을 감지
2. 남은 복제본으로 계속 서비스
3. 팔로워가 복구되면 log를 통해 따라잡음(catch up)
4. 너무 많이 뒤처진 경우 snapshot 전송

### 네트워크 분할(Network Partition)

- 과반수를 가진 파티션은 계속 서비스
- 소수(minority) 파티션은 읽기 전용(read-only)이 됨
- 네트워크가 복구되면 자동으로 복구

## 클러스터 관리

### 노드 추가

```bash
# Start new storage node
byoridb-storage --join cluster-addr:port
```

클러스터는 자동으로 다음을 수행합니다.
1. 새 노드에 파티션 할당
2. 데이터 리밸런싱
3. 파티션 맵 갱신

### 노드 제거

```bash
# Graceful removal
byoridb-admin node remove <node-id>
```

1. 다른 노드로 파티션 마이그레이션
2. 복제 완료를 대기
3. 클러스터에서 제거

### 모니터링

모니터링해야 할 주요 지표:

- `raft_leader_changes` - 리더십 변경
- `raft_log_entries` - 로그 크기
- `partition_status` - 파티션별 상태
- `replication_lag` - 팔로워 지연

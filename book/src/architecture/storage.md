# 스토리지 엔진

ByoriDB는 순수 Rust로 구현된 임베디드 키-값 저장소인 **redb**를 기반
스토리지 엔진으로 사용합니다. C++ 툴체인 의존성이 없습니다.

## redb 아키텍처

redb는 단일 파일 기반의 copy-on-write **B-tree** 저장소로, 완전한 ACID
트랜잭션과 MVCC를 지원합니다. LSM 트리가 아닙니다.

```
┌─────────────────────────────────────────────┐
│              Write Path                      │
│  begin_write → insert/remove → commit        │
│   (single writer, serialized; fsync on commit)│
└─────────────────────────────────────────────┘
                        ↓
┌─────────────────────────────────────────────┐
│            Copy-on-write B-tree              │
│  Pages are versioned; readers see a stable   │
│  MVCC snapshot while a writer commits.        │
│  Free pages are reclaimed automatically.      │
└─────────────────────────────────────────────┘
```

모든 행(row)은 원시 바이트(raw bytes)를 키로 하는 단일 redb 테이블(`"kv"`)에
저장되며, prefix scan은 정렬된 키 공간(keyspace)에 대한 범위 쿼리로 처리됩니다.

## 키 인코딩

### Vertex 키

```
[space_id:4][partition:4][tag_id:4][vid:8]
```

### Edge 키

```
[space_id:4][partition:4][edge_type:4][src_vid:8][rank:8][dst_vid:8]
```

### 값 인코딩

```
[schema_version:4][null_bitmap:N][field_values:...]
```

schema version은 온라인 스키마 변경 시 지연(lazy) 마이그레이션을 가능하게 합니다.

## 성능 튜닝

redb는 노출하는 설정 표면이 작습니다. 주요 조정 항목은 page cache 크기입니다.

```toml
[storage]
cache_size = "256MB"  # redb page cache; increase for read-heavy workloads
```

내구성(Durability)은 기본값이 `Immediate`로, 모든 commit이 fsync되고 체크섬으로
검증되어 별도의 write-ahead log 없이도 크래시 안전성을 제공합니다. (redb에는 LSM의
memtable/bloom-filter/compression 조정 항목이 없습니다. 그것들은 RocksDB 고유의
기능이었습니다.)

## 데이터 레이아웃

### Vertex 저장

```
Tag Data:
┌─────────────────────────────────────────────┐
│  Key: space|part|tag|vid                    │
│  Value: version|nulls|name|age|...          │
└─────────────────────────────────────────────┘
```

### Edge 저장

효율적인 탐색(traversal)을 위해 Edge는 양방향으로 저장됩니다.

```
Out-Edge:
┌─────────────────────────────────────────────┐
│  Key: space|part|edge|src|rank|dst          │
│  Value: version|nulls|properties...         │
└─────────────────────────────────────────────┘

In-Edge (for reverse traversal):
┌─────────────────────────────────────────────┐
│  Key: space|part|edge|dst|rank|src          │
│  Value: (same as out-edge)                  │
└─────────────────────────────────────────────┘
```

### Index 저장

```
┌─────────────────────────────────────────────┐
│  Key: space|index_id|property_value|vid     │
│  Value: (empty or additional data)          │
└─────────────────────────────────────────────┘
```

## 스키마 버전 처리

온라인 스키마 변경을 위해 스토리지 계층은 여러 스키마 버전을 처리합니다.

```
Read Path:
1. Read row from the KV store
2. Extract schema_version from row
3. If version < current:
   - Decode with old schema
   - Transform to current schema
   - Return transformed data
4. If version == current:
   - Decode directly
   - Return data
```

이러한 지연(lazy) 마이그레이션 방식은:
- 스키마 변경 중 다운타임 없음
- 다음 쓰기 시점에 행이 갱신됨
- 시간이 지나면서 점진적으로 마이그레이션됨

## 공간 회수(Space Reclamation)

redb에는 LSM compaction이 없습니다. copy-on-write B-tree이므로 free page를 추적하고
이후 쓰기 시 자동으로 재사용하므로, 삭제된 키의 공간은 별도의 백그라운드 compaction
프로세스 없이 회수됩니다.

## 스냅샷

특정 시점(point-in-time)의 일관된 스냅샷:

```bash
# Create snapshot
byoridb-admin snapshot create --space my_space

# List snapshots
byoridb-admin snapshot list

# Restore from snapshot
byoridb-admin snapshot restore --id <snapshot_id>
```

스냅샷은 단일 redb 파일에 대해 읽기 트랜잭션(MVCC 스냅샷)을 열고 이를 독립적인
백업 파일로 복사하여 생성됩니다.

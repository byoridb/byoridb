# 스토리지 엔진

[English](../../architecture/storage.html) | **한국어**

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

현재 상태는 원시 바이트 키를 쓰는 redb `kv` 테이블에 저장되며, bitemporal 버전은
별도 `history` 테이블에 저장됩니다. prefix scan은 정렬된 키 공간(keyspace)에 대한
범위 쿼리로 처리됩니다.

## 키 인코딩

### Vertex 키

```text
{space}:vertex:{vid}
```

### Edge 키

```text
{space}:edge:{src}:{edge_type}:{dst}:{ranking}
{space}:in-edge:{dst}:{edge_type}:{src}:{ranking}
```

### History 키

```text
entity_key || 0x00 || desc(valid_from) || desc(transaction_time)
```

값은 `valid_to`와 해당 시점의 binary payload이며, 삭제는 빈 payload tombstone으로
기록됩니다. current view 변경과 history append는 한 redb write transaction으로
커밋됩니다.

### 값 인코딩

```
[0xCA magic][protobuf VertexData 또는 EdgeData]
```

현재 graph vertex/edge 쓰기 경로는 magic byte가 붙은 Protocol Buffers를 사용하며, 읽기는
이전 JSON record도 fallback으로 디코딩합니다. `byoridb-codec`에는 별도의
schema-versioned row codec도 있지만 위 current-view graph record 형식과는 구분됩니다.

## 성능 튜닝

redb는 노출하는 설정 표면이 작습니다. 주요 조정 항목은 page cache 크기입니다.

```bash
export BYORIDB_CACHE_SIZE_MB=256  # redb page cache; increase for read-heavy workloads
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
│  Key: {space}:vertex:{vid}                  │
│  Value: 0xCA + protobuf VertexData          │
└─────────────────────────────────────────────┘
```

### Edge 저장

효율적인 탐색(traversal)을 위해 Edge는 양방향으로 저장됩니다.

```
Out-Edge:
┌─────────────────────────────────────────────┐
│  Key: {space}:edge:{src}:{type}:{dst}:{rank}│
│  Value: 0xCA + protobuf EdgeData            │
└─────────────────────────────────────────────┘

In-Edge (for reverse traversal):
┌─────────────────────────────────────────────┐
│  Key: {space}:in-edge:{dst}:{type}:{src}:{rank}│
│  Value: same denormalized EdgeData           │
└─────────────────────────────────────────────┘
```

### Index 저장

```
┌─────────────────────────────────────────────┐
│  Key: space|index_id|property_value|vid     │
│  Value: (empty or additional data)          │
└─────────────────────────────────────────────┘
```

## 스키마 버전 row codec

`byoridb-codec`의 row codec은 schema version을 읽어 이전 row를 현재 schema로 변환하는
기능을 제공합니다.

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

이는 codec 수준의 기능입니다. 현재 graph vertex/edge current-view record는 위의
Protocol Buffers 형식을 사용하므로 모든 record가 이 row 경로를 거치거나 자동으로
재작성된다고 가정하면 안 됩니다.

## 공간 회수(Space Reclamation)

redb에는 LSM compaction이 없습니다. copy-on-write B-tree이므로 free page를 추적하고
이후 쓰기 시 자동으로 재사용하므로, 삭제된 키의 공간은 별도의 백그라운드 compaction
프로세스 없이 회수됩니다.

## 스냅샷

`byoridb-backup`은 source redb에 MVCC read snapshot을 열고 current view와 history
테이블을 새 redb 파일로 복사합니다.

```bash
byoridb-backup create --db /var/lib/byoridb --backup-dir /backup/byoridb
byoridb-backup list --backup-dir /backup/byoridb
byoridb-backup restore --backup-dir /backup/byoridb \
  --backup-id <backup_id> --target /var/lib/byoridb-restored
```

space 단위 백업, 증분 백업과 WAL 기반 point-in-time recovery는 지원하지 않습니다.

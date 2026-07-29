# 스토리지 엔진

[English](../../architecture/storage.html)

ByoriDB는 production embedded key-value engine으로
[redb](https://www.redb.org/)를 사용합니다. redb는 pure-Rust copy-on-write
B-tree이며 ACID transaction과 MVCC read를 제공합니다. LSM tree가 아니며 ByoriDB는
RocksDB 방식의 WAL, memtable, Bloom filter, compression, compaction 설정을 노출하지
않습니다.

## 파일과 table

설정한 data path는 directory입니다. `RedbKVStore`는 다음 파일을 열거나 만듭니다.

```text
<data-path>/data.redb
```

현재는 설정된 data path 중 첫 번째 경로만 엽니다. database에는 두 개의 주요 table이
있습니다.

| Table | 용도 |
|---|---|
| `kv` | 현재 graph 상태, schema, index, user, materialized ontology 상태 |
| `history` | 변경 불가능한 asserted vertex/edge version과 삭제 tombstone |

history를 별도 B-tree에 두어 일반 current-view prefix scan이 증가하는 history와 tree
page를 공유하지 않게 합니다.

## 논리 keyspace

standalone executor는 flat `kv` table에 byte key를 저장합니다. 주요 논리 namespace는
다음과 같습니다.

```text
space:<space>                              # space metadata
space:<space>:tag:<tag>                    # tag schema
space:<space>:edge:<edge-type>             # edge schema
<space>:vertex:<vid>                       # current vertex
<space>:edge:<src>:<type>:<dst>:<rank>     # current outgoing edge
<space>:in-edge:<dst>:<type>:<src>:<rank>  # reverse-edge index
<space>:tagvid:<tag>:<vid>                 # tag membership index
__user_<username>                          # durable non-root user
```

추가 namespace에는 secondary index, degree counter, vector, ontology
materialization, inference provenance가 저장됩니다. 이것은 내부 형식이며 안정적인 public
storage API가 아닙니다.

새 vertex/edge payload는 magic prefix가 있는 protobuf encoding을 사용합니다.
`VertexCodec`은 legacy record를 위한 JSON decode fallback을 유지합니다. 일반 row
codec에도 version-aware row 지원이 있지만 standalone graph DML 경로는 vertex/edge
payload를 `VertexCodec`으로 저장합니다. 따라서 그 경로에서 실행되지 않는 자동 on-read
schema migration을 운영자가 가정해서는 안 됩니다.

## Transaction 동작

redb는 concurrent MVCC reader를 허용하고 writer를 직렬화합니다. 기본 durability에서
각 ByoriDB write transaction은 redb `Durability::Immediate`로 commit됩니다.

executor는 multi-row insert를 batch로 처리합니다. temporal `batch_apply` 연산은 하나의
redb write transaction에서 `kv`와 `history` table을 모두 열기 때문에, 그 호출에 전달한
current-view entity 변경과 history version은 함께 commit되거나 함께 실패합니다. delete는
동일한 연산에서 빈 payload tombstone을 추가합니다.

하지만 일반 transaction 계층은 아닙니다.

- `BEGIN`/`COMMIT` 쿼리 문법이 없습니다.
- compound statement의 clause는 순차 실행되며 rollback되지 않습니다.
- inference나 auxiliary maintenance 같은 일부 상위 후속 작업은 별도 storage 연산으로
  실행될 수 있습니다.

## Temporal 모델

asserted vertex/edge DML에 대해 ByoriDB는 현재 record와 append-only history version을
보존합니다.

```text
history key   = entity-key + valid-from(desc) + transaction-time(desc)
history value = valid-to + encoded entity payload
```

현재 temporal 기능의 경계는 다음과 같습니다.

- valid time과 transaction time은 하나의 monotonic epoch-millisecond 값으로 지정됩니다.
- 같은 wall-clock millisecond의 여러 쓰기도 서로 다른 증가 transaction 값을 받습니다.
- insert/update는 열린 `[timestamp, infinity)` version을 기록합니다.
- delete는 빈 tombstone을 기록합니다.
- `FETCH PROP ON <tag> <vid> AS OF <epoch-ms>`로 과거 vertex를 조회합니다.
- `FETCH PROP ON <edge-or-*> <src>-><dst> AS OF <epoch-ms>`로 이후 삭제된 edge를
  포함한 과거 edge를 조회합니다.
- 하나의 `AS OF` 값이 valid time과 transaction time 양쪽에 적용됩니다.

사용자 지정 `VALID FROM`/`VALID TO`, `BETWEEN`, temporal `GO`/`MATCH`, 과거
inferred-fact 재구성은 구현되어 있지 않습니다. history는 asserted vertex/edge 상태를
위한 것이며 ontology inference는 계속 current view를 사용합니다.

## Durability와 cache

서버는 structured `BYORIDB__...` 설정과 별도로 두 storage 환경변수를 제공합니다.

| 변수 | 기본값 | 의미 |
|---|---:|---|
| `BYORIDB_CACHE_SIZE_MB` | `256` | redb page cache 크기(MiB). 0 이하 또는 잘못된 값은 기본값 사용 |
| `BYORIDB_DURABILITY` | immediate | `none`, `relaxed`, `eventual`이면 bulk-load용 relaxed durability 사용 |

relaxed durability는 대부분의 commit에서 fsync를 건너뛰고 주기적으로 checkpoint합니다.
crash 시 최근 commit을 잃을 수 있으므로 일반 serving이 아니라 다시 load할 수 있는
데이터에만 사용하세요.

graceful shutdown 시 서버는 Immediate 빈 commit을 수행해 redb allocator 상태를 clean하게
남깁니다. query drain과 checkpoint를 마칠 수 있도록 충분한 종료 시간을 제공해야 합니다.

## Backup 영향

backup 구현은 read transaction에서 `kv`와 `history`를 모두 새 redb 파일로 복사합니다.
변경 중인 `data.redb`를 단순 복사하거나 current table만 보존하는 방식은 지원되는
대체 방법이 아닙니다. [백업 및 복원](../operations/backup.html) 절차를 따르고 정기적으로
restore를 시험하세요.

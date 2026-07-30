[English](../../reference/performance.html)

# 성능

ByoriDB는 안정적인 QPS 또는 latency 표를 게시하지 않습니다. 결과는 dataset,
query shape, redb file, durability, cache warmth, filesystem, hardware에 따라
달라집니다. Capacity planning에 사용하는 수치는 대상 build와 workload에서
재현 가능한 실행으로 얻어야 합니다.

## 저장소의 benchmark 도구

### Criterion microbenchmark

`benches/benchmark.rs`는 object 생성, parsing, plan building, serialization,
filter, arena allocation을 측정합니다.

```bash
cargo bench --locked -p byoridb --bench benchmark
```

### In-process end-to-end benchmark

`benches/e2e_benchmark.rs`는 temporary redb database를 만들고 batch insert,
`FETCH`, `GO`, `LOOKUP`, 전체 query-service 호출을 실행합니다.

```bash
cargo bench --locked -p byoridb --bench e2e_benchmark
```

이 실행에는 network latency나 production 크기의 persistent database가 포함되지
않습니다. 결과를 보고할 때 Criterion output, commit SHA, Rust version, OS, CPU,
memory, filesystem, redb file size, cache 설정, durability를 함께 보존하세요.

### gRPC load generator

client package에는 단순 concurrent load generator가 있습니다.

```bash
export BYORIDB_USER=root
export BYORIDB_PASSWORD='same-secret-used-to-start-the-server'

cargo run --locked --release -p byoridb-client --bin load_test -- \
  --address http://127.0.0.1:9669 \
  --concurrency 20 \
  --duration 30 \
  --setup 'USE example' \
  --query 'FETCH PROP ON person 1'
```

request 수, error, 평균/초별 QPS를 보고합니다. Latency-distribution benchmark가
아니며 test data를 provision하지 않습니다.

## Tuning 전 query 검사

`EXPLAIN`은 statement를 실행하지 않고 logical operator tree와 선택한 access path를
보고합니다.

```sql
EXPLAIN MATCH (p:person) WHERE p.name == "Alice" RETURN p;
```

`access` column은 named index, tag-VID index, point lookup,
edge-prefix/reverse-edge access, full scan을 구분합니다.

`PROFILE`은 query를 실행하고 instrument된 지점의 observation을 overlay합니다.

```sql
PROFILE GO FROM 1 OVER knows YIELD dst(edge);
PROFILE MATCH (p:person) RETURN count(p);
```

column은 `id`, `operator`, `rows`, `time(us)`, `access`, `detail`입니다. Executor는
Volcano iterator tree가 아니라 imperative 방식이므로 모든 operator의 시간을
독립적으로 나눌 수는 없습니다. Timing이 없으면 0이 아니라 unmeasured입니다.

## Query 사용 방법

### VID를 알면 point access 사용

```sql
FETCH PROP ON person 42;
```

`FETCH`는 정확한 current-view key를 만들고 여러 VID에 batch get을 사용합니다.
일반 `MATCH`에는 더 많은 planning과 scan/expansion 작업이 있습니다.

### Secondary index 생성과 사용

```sql
CREATE TAG INDEX person_name_idx ON person(name);
LOOKUP ON person WHERE person.name == "Alice";
```

현재 secondary-index fast path는 single-field equality predicate만 추출합니다.
`<`, `<=`, `>`, `>=`, `!=` predicate는 tag scan에서 평가되며 range index scan을
사용하지 않습니다. `EXPLAIN`으로 실제 access path를 확인하세요.

label-only `MATCH`는 자동 유지되는 tag-VID index를 사용할 수 있습니다. 해당 index
entry 도입 전에 생성된 오래된 data는 backfill/reload 전까지 full-scan fallback을
사용할 수 있습니다.

### Result set 제한

```sql
MATCH (p:person) RETURN p LIMIT 100;
```

aggregate 또는 일부 property만 필요할 때 전체 vertex payload를 반환하지 마세요.
result materialization은 memory bound이며 application guard는 추정치이므로 bounded
query를 대체하지 못합니다.

### Batch write

```sql
INSERT VERTEX person(name, age) VALUES
  1:("Alice", 30),
  2:("Bob", 25),
  3:("Carol", 28);
```

하나의 multi-row statement를 사용하면 statement별 transaction 대신 redb batch
하나를 commit할 수 있습니다. `batch_apply`에 전달된 current entity write와 대응
temporal history version은 함께 commit됩니다.

### 올바른 traversal 방향 사용

outgoing traversal은 source VID로 제한된 edge prefix를 scan합니다.
incoming/undirected traversal은 전체 edge scan 대신 유지되는 reverse-edge index를
사용합니다. 큰 degree와 variable-length path는 여전히 빠르게 확장될 수 있으므로
좁은 edge type, bounded step range, filter, limit을 사용하세요.

## Runtime guard

기본 execution context는 다음 guard를 적용합니다.

| Guard | 기본값 | 설정 |
|---|---:|---|
| 추정 result-memory budget | 1024 MiB | `BYORIDB_MAX_MEMORY_MB`; `0`이면 비활성 |
| guarded prefix scan 반환 row | 100,000 | `BYORIDB_MAX_SCAN_LIMIT`; `0`이면 비활성 |
| traversal/materialization 방문 node | 100,000 | 내부 execution 기본값 |
| 최대 GO/MATCH path step | 20 | 내부 execution 기본값 |
| 최대 shortest path 열거 수 | 1,024 | 내부 execution 기본값 |

일부 traversal algorithm은 cap에서 warning 후 partial/truncated result를 반환하고,
너무 큰 step range는 error를 반환합니다. Guard를 높이거나 끄면 incomplete
analytical query가 out-of-memory process failure로 바뀔 수 있습니다. 하나씩
바꾸며 process/PVC metric을 관찰하세요.

`ExecutionConfig`의 `timeout_ms` field는 현재 일반 server-side query timeout으로
강제되지 않습니다. 외부 timeout을 신중히 적용하세요. HTTP client timeout이
server에서 이미 실행 중인 work를 반드시 cancel하지는 않습니다. 인증된
active-query diagnostics로 확인하세요.

## redb tuning

### Page cache

```bash
export BYORIDB_CACHE_SIZE_MB=4096
```

기본값은 256 MiB입니다. Process가 cache 외에도 query materialization, index, OS를
위한 headroom이 있을 때만 늘리세요. Cold/warm run을 따로 측정하세요.

### Durability

일반 serving은 write transaction마다 fsync하는 Immediate durability를 사용합니다.
`BYORIDB_DURABILITY=none`, `relaxed`, `eventual`은 crash 시 최근 commit을 잃을 수
있는 빠른 bulk-load mode를 활성화합니다. Idempotent하고 다시 load할 수 있는
import에만 사용하고 이후 기본값으로 돌아가세요.

지원되는 `block_cache_size`, `write_buffer_size`, compression, compaction
configuration key는 없습니다. 이것은 ByoriDB의 redb tuning surface가 아니라
LSM/RocksDB 개념입니다.

## Temporal read 비용

- current read는 `kv` table에 남아 history를 scan하지 않습니다.
- vertex `AS OF`는 ordered history key로 요청한 `(valid_at, transaction_at)` 근처를
  seek한 뒤 조건에 맞는 version을 확인합니다.
- edge `AS OF`는 source/edge-type prefix 아래 historical entity key를 먼저 열거한
  뒤 각 candidate를 resolve합니다. 비용은 해당 prefix 아래 과거 edge identity
  수와 함께 증가합니다.
- history는 asserted-fact-only입니다. Inference와 일반 traversal은 current view를
  사용합니다.

retention 증가와 historical edge workload를 명시적으로 test하세요. 자동 history
pruning policy는 현재 구현되지 않았습니다.

## 다른 경계

- redb는 write transaction을 직렬화하므로 client concurrency를 늘려도 parallel
  disk writer가 생기지 않습니다.
- gRPC gzip/zstd support는 일부 network payload를 줄이지만 storage compression이
  아닙니다.
- persisted HNSW는 executor의 vector-count threshold를 넘을 때만 사용하며 작은
  집합은 exact flat cosine search를 사용합니다.
- 여러 node처럼 보이는 Compose configuration은 distributed performance topology가
  아닙니다. Independent database로만 benchmark하세요.

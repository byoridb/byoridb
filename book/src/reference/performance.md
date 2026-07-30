# 성능

성능 특성 및 튜닝 가이드라인입니다.

## 벤치마크

환경과 dataset이 없는 고정 latency/QPS 표는 제거했습니다. 재현 가능한 최신 명령과
측정값은 [docs/PLAN.md](https://github.com/byoridb/byoridb/blob/main/docs/PLAN.md)의
측정 환경/벤치 기반선을 기준으로 합니다.
multi-node launcher가 완성되지 않았으므로 3노드 처리량을 검증된 수치로 제시하지 않습니다.

## 튜닝 가이드라인

### 메모리 튜닝

#### redb page cache와 쿼리 메모리

읽기 중심 워크로드에서는 노드 메모리 범위 안에서 `BYORIDB_CACHE_SIZE_MB`를 조정합니다.
대형 결과의 OOM을 막는 `BYORIDB_MAX_MEMORY_MB`와 scan cap인
`BYORIDB_MAX_SCAN_LIMIT`도 함께 고려하세요.

### 쿼리 튜닝

#### 인덱스 사용

```sql
-- Without index: full scan
LOOKUP ON person WHERE person.name == 'Alice';

-- With index: fast lookup
CREATE TAG INDEX name_idx ON person(name);
LOOKUP ON person WHERE person.name == 'Alice';
```

#### 결과 제한

```sql
-- Avoid unbounded queries
MATCH (n:person) RETURN n;          -- May return millions

-- Use LIMIT
MATCH (n:person) RETURN n LIMIT 100;
```

#### 알려진 VID에는 FETCH 사용

```sql
-- If you know the vertex ID, use FETCH
FETCH PROP ON person 1;

-- Instead of
MATCH (n:person) WHERE id(n) == 1 RETURN n;
```

### 스토리지 튜닝

redb는 copy-on-write B-tree라 RocksDB식 compression, write-buffer, background
compaction 설정을 제공하지 않습니다. 기본 durability는 commit마다 fsync하는
`immediate`입니다. `BYORIDB_DURABILITY=relaxed`는 최근 commit 손실을 감수할 수 있는
재적재 가능한 bulk import에서만 사용하세요.

### 배치 쓰기

```sql
-- Instead of individual inserts
INSERT VERTEX person VALUES 1:('Alice', 30);
INSERT VERTEX person VALUES 2:('Bob', 25);

-- Use batch insert
INSERT VERTEX person VALUES
    1:('Alice', 30),
    2:('Bob', 25),
    3:('Carol', 28);
```

## 프로파일링

### 쿼리 프로파일링

```sql
PROFILE GO FROM 1 OVER follow YIELD $$.person.name;
```

출력 컬럼은 `id`, `operator`, `rows`, `time(us)`, `access`, `detail`입니다.

### 실행 계획 설명(Explain Plan)

```sql
EXPLAIN MATCH (a:person)-[e:follow]->(b:person)
WHERE a.name == 'Alice'
RETURN b.name;
```

## 성능 모니터링

주시해야 할 주요 지표:

- `byoridb_query_latency_seconds` - 쿼리 지연 시간
- `byoridb_query_errors_total` - query 오류 수
- `byoridb_slow_queries_total` - slow threshold 초과 수
- `byoridb_inflight_queries` - 현재 실행 중인 query 수
- `byoridb_rows_written_total` - operation별 기록 행 수

`byoridb_storage_bytes`와 partition metric 갱신 함수는 아직 standalone 경로에 연결되지
않았습니다. 디스크 사용량과 container resource는 외부 exporter로 수집하세요.

## 자주 발생하는 문제

### 높은 지연 시간

1. working set 대비 redb page cache 크기 확인
2. full scan 여부 확인 (EXPLAIN 사용)
3. 쿼리 패턴에 맞는 인덱스가 존재하는지 검증

### 낮은 처리량

1. CPU 사용률 확인
2. full scan과 결과 materialization 크기 확인
3. 배치 작업 사용

### 높은 디스크 사용량

1. `history` 보존량과 불필요한 데이터 확인
2. 별도 backup 디렉터리 보존 정책 확인
3. 리텐션/GC가 아직 미구현임을 용량 계획에 반영

# 성능

성능 특성 및 튜닝 가이드라인입니다.

## 벤치마크

### 테스트 환경

- CPU: 8-core AMD EPYC
- Memory: 32 GB
- Storage: NVMe SSD
- Dataset: 10M vertices, 100M edges

### 쿼리 성능

| 쿼리 유형 | p50 | p95 | p99 |
|------------|-----|-----|-----|
| FETCH single vertex | 0.2ms | 0.5ms | 1ms |
| FETCH batch (100) | 2ms | 5ms | 10ms |
| GO 1-hop | 1ms | 3ms | 5ms |
| GO 2-hop | 10ms | 30ms | 50ms |
| MATCH simple | 5ms | 15ms | 30ms |
| LOOKUP indexed | 1ms | 3ms | 5ms |

### 처리량(Throughput)

| 작업 | 단일 노드 | 3노드 클러스터 |
|-----------|-------------|----------------|
| Point reads | 50K QPS | 150K QPS |
| Point writes | 20K QPS | 15K QPS |
| Mixed workload | 30K QPS | 80K QPS |

## 튜닝 가이드라인

### 메모리 튜닝

#### Block Cache

읽기 중심 워크로드에서는 늘리세요.

```toml
[storage]
block_cache_size = "4GB"  # 25% of available memory
```

#### Write Buffer

쓰기 중심 워크로드에서는 늘리세요.

```toml
[storage]
write_buffer_size = "128MB"
max_write_buffer_number = 4
```

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

#### 압축(Compression)

CPU를 소비하는 대신 디스크 공간을 절약합니다.

```toml
[storage]
compression = "lz4"  # Good balance
# compression = "zstd"  # More compression, more CPU
```

#### Compaction

워크로드에 맞게 조정하세요.

```toml
[storage]
# Write-heavy: more compaction threads
max_background_compactions = 4

# Read-heavy: smaller files for faster seeks
target_file_size_base = "32MB"
```

### 네트워크 튜닝

#### 커넥션 풀링

```toml
[client]
connection_pool_size = 10
connection_timeout_ms = 5000
```

#### 배치 작업

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
PROFILE {
    GO FROM 1 OVER follow YIELD $$.person.name;
}
```

출력:
```
+------------------+----------+-------+
| Operator         | Time(ms) | Rows  |
+------------------+----------+-------+
| GetNeighbors     | 2.5      | 100   |
| Project          | 0.3      | 100   |
+------------------+----------+-------+
Total: 2.8ms
```

### 실행 계획 설명(Explain Plan)

```sql
EXPLAIN {
    MATCH (a:person)-[e:follow]->(b:person)
    WHERE a.name == 'Alice'
    RETURN b.name;
}
```

## 성능 모니터링

주시해야 할 주요 지표:

- `byoridb_query_latency_seconds` - 쿼리 지연 시간
- `byoridb_storage_bytes` - 스토리지 사용량
- `byoridb_partition_hotspot_ratio` - 파티션 편향(skew)

## 자주 발생하는 문제

### 높은 지연 시간

1. 캐시 적중률 확인 (90% 이상이어야 함)
2. full scan 여부 확인 (EXPLAIN 사용)
3. 쿼리 패턴에 맞는 인덱스가 존재하는지 검증

### 낮은 처리량

1. CPU 사용률 확인
2. 커넥션 풀 크기 검증
3. 배치 작업 사용

### 높은 디스크 사용량

1. 압축 활성화
2. 오래된 스냅샷 확인
3. compaction이 동작 중인지 검증

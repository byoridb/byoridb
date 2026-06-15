# 모니터링

성능과 상태를 위해 ByoriDB를 모니터링하세요.

## 메트릭 엔드포인트

ByoriDB는 Prometheus 호환 메트릭을 노출합니다:

```bash
curl http://localhost:19669/metrics
```

## 주요 메트릭

### 쿼리 메트릭

| 메트릭 | 타입 | 설명 |
|--------|------|-------------|
| `byoridb_query_total` | Counter | 실행된 전체 쿼리 수 |
| `byoridb_query_latency_seconds` | Histogram | 쿼리 지연 시간 |
| `byoridb_query_errors_total` | Counter | 실패한 쿼리 수 |

### 스토리지 메트릭

| 메트릭 | 타입 | 설명 |
|--------|------|-------------|
| `byoridb_storage_bytes` | Gauge | 사용 중인 전체 스토리지 |

### 파티션 메트릭

| 메트릭 | 타입 | 설명 |
|--------|------|-------------|
| `byoridb_partition_requests_total` | Counter | 파티션 요청 수 |
| `byoridb_partition_hotspot_ratio` | Gauge | 핫스팟 비율 |
| `byoridb_partition_count` | Gauge | 파티션 개수 |
| `byoridb_partition_leader_count` | Gauge | 리더 파티션 개수 |

### 세션 메트릭

| 메트릭 | 타입 | 설명 |
|--------|------|-------------|
| `byoridb_active_connections` | Gauge | 활성 연결 수 |
| `byoridb_active_sessions` | Gauge | 활성 세션 수 |

## Prometheus 설정

```yaml
# prometheus.yml
scrape_configs:
  - job_name: 'byoridb'
    static_configs:
      - targets:
        - 'graph1:19669'
        - 'graph2:19669'
    metrics_path: /metrics
    scrape_interval: 15s
```

## Grafana 대시보드

ByoriDB 대시보드를 가져옵니다:

1. Grafana → Dashboards → Import으로 이동합니다
2. 대시보드 ID를 입력하거나 JSON을 업로드합니다
3. Prometheus 데이터 소스를 선택합니다

### 필수 패널

**쿼리 성능:**
- 쿼리 처리율 (queries/sec)
- 쿼리 지연 시간 (p50, p95, p99)
- 오류율

**스토리지 상태:**
- 디스크 사용량
- 캐시 적중률
- 컴팩션 상태

**클러스터 상태:**
- 리더 분포
- 복제 지연(replication lag)
- 노드 상태

## 알림(Alerting)

### Prometheus 알림 규칙

```yaml
# alerts.yml
groups:
  - name: byoridb
    rules:
      - alert: HighQueryLatency
        expr: histogram_quantile(0.99, byoridb_query_latency_seconds) > 1
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "High query latency detected"

      - alert: HighPartitionHotspotRatio
        expr: byoridb_partition_hotspot_ratio > 0.8
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "Partition hotspot ratio is high"

      - alert: StorageGrowingQuickly
        expr: rate(byoridb_storage_bytes[10m]) > 0
        for: 10m
        labels:
          severity: warning
        annotations:
          summary: "Storage usage is increasing"

      - alert: HighErrorRate
        expr: rate(byoridb_query_errors_total[5m]) / rate(byoridb_query_total[5m]) > 0.01
        for: 5m
        labels:
          severity: critical
        annotations:
          summary: "Error rate exceeds 1%"
```

## 로깅

### 로그 설정

```toml
[logging]
level = "info"           # trace, debug, info, warn, error
format = "json"          # json or text
output = "/var/log/byoridb/byoridb.log"
```

### 로그 레벨

| 레벨 | 사용 사례 |
|-------|----------|
| error | 조치가 필요한 장애 |
| warn | 잠재적 문제 |
| info | 정상 운영 |
| debug | 문제 해결 |
| trace | 상세 디버깅 |

### 구조화된 로깅

```json
{
  "timestamp": "2024-01-15T10:30:00Z",
  "level": "info",
  "target": "byoridb_graph::executor",
  "message": "Query executed",
  "query_id": "abc123",
  "duration_ms": 45,
  "rows_returned": 100
}
```

## 헬스 체크

### Liveness Probe

```bash
curl -f http://localhost:19669/health/live
```

### Readiness Probe

```bash
curl -f http://localhost:19669/health/ready
```

### Kubernetes Probe

```yaml
livenessProbe:
  httpGet:
    path: /health/live
    port: 19669
  initialDelaySeconds: 30
  periodSeconds: 10

readinessProbe:
  httpGet:
    path: /health/ready
    port: 19669
  initialDelaySeconds: 5
  periodSeconds: 5
```

## 트레이싱

Jaeger로 분산 트레이싱을 활성화합니다:

```toml
[tracing]
enabled = true
jaeger_endpoint = "http://jaeger:14268/api/traces"
sample_rate = 0.1  # Sample 10% of requests
```

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
| `byoridb_slow_queries_total` | Counter | slow threshold를 넘은 쿼리 수 |
| `byoridb_inflight_queries` | Gauge | 현재 실행 중인 쿼리 수 |
| `byoridb_rows_written_total` | Counter | INSERT/UPDATE/DELETE가 기록한 행 수 |

### 아직 연결되지 않은 메트릭

`byoridb_storage_bytes`, `byoridb_active_connections`, `byoridb_active_sessions`와 partition
계열 metric 이름/갱신 함수는 코드에 정의돼 있지만 standalone 실행 경로에는 호출이
연결돼 있지 않습니다. 현재 값이 수집되는 운영 metric으로 가정하거나 alert에 사용하면
안 됩니다. 디스크·컨테이너 상태는 node/container exporter 같은 외부 수집기를 사용하세요.

## Prometheus 설정

```yaml
# prometheus.yml
scrape_configs:
  - job_name: 'byoridb'
    static_configs:
      - targets:
        - 'byoridb:19669'
    metrics_path: /metrics
    scrape_interval: 15s
```

## Grafana 대시보드

저장소에는 아직 공식 Grafana dashboard JSON이 없습니다. 아래 패널은 `/metrics`를
수집한 뒤 운영자가 구성할 권장 목록입니다.

### 필수 패널

**쿼리 성능:**
- 쿼리 처리율 (queries/sec)
- 쿼리 지연 시간 (p50, p95, p99)
- 오류율
- slow query, in-flight query, operation별 rows written

디스크 사용량, bitemporal history 증가율, 복제 지연과 리더 분포는 현재 `/metrics`에서
측정되지 않습니다. 별도 exporter나 기능 wiring을 추가하기 전에는 dashboard 근거로
사용할 수 없습니다.

## 알림(Alerting)

### Prometheus 알림 규칙

공식 alert rule 파일도 아직 제공하지 않습니다. 다음은 시작점일 뿐이며 실제 노출
label과 workload SLO에 맞춰 검증한 뒤 사용해야 합니다.

```yaml
# alerts.yml
groups:
  - name: byoridb
    rules:
      - alert: HighQueryLatency
        expr: histogram_quantile(0.99, sum by (le) (rate(byoridb_query_latency_seconds_bucket[5m]))) > 1
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "High query latency detected"

      - alert: HighErrorRate
        expr: sum(rate(byoridb_query_errors_total[5m])) / clamp_min(sum(rate(byoridb_query_total[5m])), 1) > 0.01
        for: 5m
        labels:
          severity: critical
        annotations:
          summary: "Error rate exceeds 1%"
```

## 로깅

### 로그 설정

로그 레벨과 모듈 필터는 `RUST_LOG`로 설정합니다. standalone launcher의 출력 형식은
현재 text formatter로 고정되어 있고 `BYORIDB_LOG_FORMAT` 환경변수나 JSON formatter는
연결돼 있지 않습니다. 파일 출력, Filebeat/Fluentd 등 중앙 수집 설정도 bundled 상태가
아니므로 container runtime 또는 운영 플랫폼에서 stdout을 수집해야 합니다.

### 로그 레벨

| 레벨 | 사용 사례 |
|-------|----------|
| error | 조치가 필요한 장애 |
| warn | 잠재적 문제 |
| info | 정상 운영 |
| debug | 문제 해결 |
| trace | 상세 디버깅 |

### 쿼리 로그 필드

쿼리 로그에는 `query_type`, `query_length_bytes`, `latency_ms`, `row_count` 같은 bounded
metadata만 기록합니다. raw query, 비밀번호, 사용자명과 bearer session ID는 기록하지
않습니다.

## 헬스 체크

### Liveness Probe

```bash
curl -f http://localhost:19669/health
```

### Readiness Probe

```bash
curl -f http://localhost:19669/ready
```

### Kubernetes Probe

```yaml
livenessProbe:
  httpGet:
    path: /health
    port: 19669
  initialDelaySeconds: 30
  periodSeconds: 10

readinessProbe:
  httpGet:
    path: /ready
    port: 19669
  initialDelaySeconds: 5
  periodSeconds: 5
```

## 트레이싱

Jaeger/OpenTelemetry exporter용 native 설정은 아직 없습니다. 현재는 text log의 구조화
필드와 Prometheus metrics를 사용하고, 분산 tracing은 후속 운영 과제로 남아 있습니다.

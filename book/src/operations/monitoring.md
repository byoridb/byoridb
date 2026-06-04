# Monitoring

Monitor ByoriDB for performance and health.

## Metrics Endpoint

ByoriDB exposes Prometheus-compatible metrics:

```bash
curl http://localhost:19669/metrics
```

## Key Metrics

### Query Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `byoridb_query_total` | Counter | Total queries executed |
| `byoridb_query_latency_seconds` | Histogram | Query latency |
| `byoridb_query_errors_total` | Counter | Failed queries |

### Storage Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `byoridb_storage_bytes` | Gauge | Total storage used |

### Partition Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `byoridb_partition_requests_total` | Counter | Partition requests |
| `byoridb_partition_hotspot_ratio` | Gauge | Hotspot ratio |
| `byoridb_partition_count` | Gauge | Partition count |
| `byoridb_partition_leader_count` | Gauge | Leader partition count |

### Session Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `byoridb_active_connections` | Gauge | Active connections |
| `byoridb_active_sessions` | Gauge | Active sessions |

## Prometheus Configuration

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

## Grafana Dashboard

Import the ByoriDB dashboard:

1. Go to Grafana → Dashboards → Import
2. Enter dashboard ID or upload JSON
3. Select Prometheus data source

### Essential Panels

**Query Performance:**
- Query rate (queries/sec)
- Query latency (p50, p95, p99)
- Error rate

**Storage Health:**
- Disk usage
- Cache hit ratio
- Compaction status

**Cluster Status:**
- Leader distribution
- Replication lag
- Node status

## Alerting

### Prometheus Alert Rules

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

## Logging

### Log Configuration

```toml
[logging]
level = "info"           # trace, debug, info, warn, error
format = "json"          # json or text
output = "/var/log/byoridb/byoridb.log"
```

### Log Levels

| Level | Use Case |
|-------|----------|
| error | Failures requiring attention |
| warn | Potential issues |
| info | Normal operations |
| debug | Troubleshooting |
| trace | Detailed debugging |

### Structured Logging

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

## Health Checks

### Liveness Probe

```bash
curl -f http://localhost:19669/health/live
```

### Readiness Probe

```bash
curl -f http://localhost:19669/health/ready
```

### Kubernetes Probes

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

## Tracing

Enable distributed tracing with Jaeger:

```toml
[tracing]
enabled = true
jaeger_endpoint = "http://jaeger:14268/api/traces"
sample_rate = 0.1  # Sample 10% of requests
```

# 모니터링

[English](../../operations/monitoring.html)

standalone server는 Prometheus text metric, 가벼운 HTTP health signal, 인증된 active-query
diagnostics를 제공합니다. log에는 Rust `tracing`을 사용합니다. bundled Grafana dashboard,
Jaeger exporter, logging configuration section은 없습니다.

## HTTP endpoint

| Endpoint | 접근 | 현재 동작 |
|---|---|---|
| `GET /health` | 인증 없음 | handler에 접근 가능하면 `200 OK`와 `OK` 반환 |
| `GET /ready` | 인증 없음 | `200 READY`, graceful shutdown 시작 후 `503 SHUTTING DOWN` |
| `GET /metrics` | 인증 없음 | Prometheus text exposition |
| `GET /api/v1/metrics` | 인증 없음 | `/metrics`를 가리키는 JSON discovery response |
| `GET /api/v1/diagnostics/queries` | GOD/ADMIN Bearer session | 민감 필드를 제거한 active query metadata |

`/health`는 liveness signal이며 redb의 deep read/write check가 아닙니다. `/ready`는 process가
새 query를 받는지를 나타내며 모든 downstream distributed component를 probe하지 않습니다.

이 endpoint를 trusted network 내부에 두세요. 특히 `/metrics`는 application에서 인증하지
않습니다.

## Query 경로가 기록하는 metric

현재 Graph execution path는 다음 series를 갱신합니다.

| Metric | Type | Label | 의미 |
|---|---|---|---|
| `byoridb_query_total` | counter | `type`, `space` | 성공적으로 끝난 query |
| `byoridb_query_latency_seconds` | histogram | `type` | 실행이 반환된 뒤 기록되는 성공 또는 execution-result error의 duration |
| `byoridb_query_errors_total` | counter | `type`, `error` | query execution이 반환한 error |
| `byoridb_slow_queries_total` | counter | `type` | 고정 1초 threshold를 넘긴 기록 대상의 성공한 execution |
| `byoridb_inflight_queries` | gauge | 없음 | 현재 실행 중인 query |
| `byoridb_rows_written_total` | counter | `op` | 성공한 insert/update/delete 결과가 보고한 row 수 |

query type label에는 `fetch`, `go`, `match`, `lookup`, `insert`, `unknown` 등이 있습니다.
기록 대상의 느린 성공 execution은 query text, duration, full scan 여부를 포함한 structured warning도 만듭니다.
credential statement는 `PASSWORD` keyword 이후가 redaction됩니다.

authentication, parsing, planning은 metric timer가 끝나기 전에 실패할 수 있습니다. 따라서
그 failure는 latency histogram과 query-error counter에 모두 포함되지 않습니다. 이 series를
거부된 모든 request의 total로 해석하지 마세요.

metrics module은 active connection/session, storage, partition series도 선언하지만 standalone
runtime은 현재 update 함수를 호출하지 않습니다. 아래 metric이 채워진다고 가정한 alert를
만들지 마세요.

```text
byoridb_active_connections
byoridb_active_sessions
byoridb_storage_bytes
byoridb_partition_requests_total
byoridb_partition_hotspot_ratio
byoridb_partition_count
byoridb_partition_leader_count
```

## Prometheus scrape 설정

```yaml
scrape_configs:
  - job_name: byoridb
    metrics_path: /metrics
    static_configs:
      - targets: ["byoridb:19669"]
```

임의 capacity target이 아니라 관찰 가능한 동작을 기반으로 alert를 시작하세요.

```yaml
groups:
  - name: byoridb
    rules:
      - alert: ByoriDBScrapeDown
        expr: up{job="byoridb"} == 0
        for: 2m
        annotations:
          summary: "Prometheus cannot scrape ByoriDB"

      - alert: ByoriDBQueryErrors
        expr: sum(rate(byoridb_query_errors_total[5m])) > 0
        for: 5m
        annotations:
          summary: "ByoriDB is returning query errors"

      - alert: ByoriDBQueriesRemainInflight
        expr: byoridb_inflight_queries > 0
        for: 15m
        annotations:
          summary: "ByoriDB has long-running in-flight work"
```

측정한 workload에 맞춰 threshold를 조정하세요. 저장소는 보편적인 latency, QPS,
storage-growth SLO를 정의하지 않습니다.

## Active-query diagnostics

HTTP session을 만든 뒤 live GOD 또는 ADMIN session ID를 Bearer token으로 전달합니다.

```bash
curl http://127.0.0.1:19669/api/v1/diagnostics/queries \
  -H "Authorization: Bearer <session-id>"
```

응답 형태 예시:

```json
{
  "count": 1,
  "queries": [
    {
      "id": 42,
      "query_type": "match",
      "query": "MATCH (n:person) RETURN n",
      "space": "example",
      "started_at_ms": 1785313593000
    }
  ]
}
```

응답은 의도적으로 session ID를 제외하고 password statement를 redaction합니다. Bearer
credential이 없거나 잘못되면 HTTP 401, 유효한 non-admin session이면 HTTP 403을
반환합니다. diagnostic 상태는 process 내부에 있고 restart 시 사라집니다.

## Logging

server는 기본 text `tracing_subscriber` formatter를 초기화합니다. `RUST_LOG`로 filtering을
제어합니다.

```bash
export RUST_LOG='info,byoridb_graph=debug,byoridb_storage=info'
byoridb-server
```

stdout/stderr를 platform log collector로 보내세요. application은 현재 `[logging]` file
section, log-file rotation, JSON-format toggle, native Jaeger/OpenTelemetry 설정을 구현하지
않습니다.

주요 운영 event에는 startup mode, redb path/cache/durability, 인증 username, query latency,
slow/full-scan warning, readiness shutdown, drain timeout, final checkpoint 상태가 있습니다.
password 값과 raw session credential은 나타나면 안 됩니다.

## Kubernetes probe

저장소의 StatefulSet은 실제 endpoint를 사용합니다.

```yaml
startupProbe:
  httpGet: { path: /health, port: 19669 }

readinessProbe:
  httpGet: { path: /ready, port: 19669 }

livenessProbe:
  httpGet: { path: /health, port: 19669 }
```

repository manifest는 큰 redb 파일을 위해 넉넉한 startup/termination budget을
사용합니다. 줄이기 전에 restore/startup drill로 다시 계산하세요. 존재하지 않는
`/health/live` 또는 `/health/ready` route를 사용하지 마세요.

## 권장 운영 확인

- scrape/readiness loss alert
- query error, latency distribution, slow query, 지속되는 in-flight work 추적
- application storage gauge가 연결되지 않았으므로 platform에서 disk/PVC usage와 process
  CPU/memory 수집
- 계획된 shutdown 중 redb checkpoint message 확인
- restore/upgrade 후 대표 authenticated read와 `AS OF` query 실행
- multi-replica session semantics가 구현될 때까지 session/role 변경을 현재 process에
  한정된 것으로 취급

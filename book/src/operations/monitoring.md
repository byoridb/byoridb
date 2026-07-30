# Monitoring

[한국어](../ko/operations/monitoring.html)

The standalone server exposes Prometheus text metrics, lightweight HTTP health
signals, and an authenticated active-query diagnostic. It uses Rust `tracing`
for logs. There is no bundled Grafana dashboard, Jaeger exporter, or logging
configuration section.

## HTTP endpoints

| Endpoint | Access | Current behavior |
|---|---|---|
| `GET /health` | unauthenticated | Returns `200 OK` and `OK` when the handler is reachable |
| `GET /ready` | unauthenticated | Returns `200 READY`, then `503 SHUTTING DOWN` after graceful shutdown begins |
| `GET /metrics` | unauthenticated | Prometheus text exposition |
| `GET /api/v1/metrics` | unauthenticated | JSON discovery response pointing to `/metrics` |
| `GET /api/v1/diagnostics/queries` | GOD/ADMIN session in `X-ByoriDB-Session-Id` | Active query metadata with sensitive fields omitted |

`/health` is a liveness signal, not a deep read/write check of redb. `/ready`
tracks whether the process accepts new queries; it does not probe every
downstream distributed component.

Keep these endpoints inside a trusted network. In particular, `/metrics` is not
authenticated by the application.

## Metrics that the query path records

The current Graph execution path updates these series:

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `byoridb_query_total` | counter | `type`, `space` | Successfully completed queries |
| `byoridb_query_latency_seconds` | histogram | `type` | Duration recorded after execution returns, for success or an execution-result error |
| `byoridb_query_errors_total` | counter | `type`, `error` | Errors returned by query execution |
| `byoridb_slow_queries_total` | counter | `type` | Successful recorded executions exceeding the fixed one-second threshold |
| `byoridb_inflight_queries` | gauge | none | Queries currently executing |
| `byoridb_rows_written_total` | counter | `op` | Rows reported by successful insert/update/delete results |

Query type labels include values such as `fetch`, `go`, `match`, `lookup`,
`insert`, and `unknown`. Recorded slow successes also produce a structured
warning with the query type, duration, full-scan flag, and query length. Raw
query text is not retained or written to that warning.

Authentication, parsing, and planning can fail before the metrics timer is
finished. Those failures are therefore absent from both the latency histogram
and query-error counter; these series are not totals for every rejected request.

The metrics module also declares active-connection, active-session, storage,
and partition series, but the standalone runtime does not currently call their
update functions. Do not build alerts that assume these are populated:

```text
byoridb_active_connections
byoridb_active_sessions
byoridb_storage_bytes
byoridb_partition_requests_total
byoridb_partition_hotspot_ratio
byoridb_partition_count
byoridb_partition_leader_count
```

## Prometheus scrape configuration

```yaml
scrape_configs:
  - job_name: byoridb
    metrics_path: /metrics
    static_configs:
      - targets: ["byoridb:19669"]
```

Start with alerts based on observable behavior rather than invented capacity
targets. For example:

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

Tune thresholds from a measured workload. The repository does not define a
universal latency, QPS, or storage-growth SLO.

## Active-query diagnostics

Create an HTTP session, then pass a live GOD or ADMIN session ID in the session
header:

```bash
curl http://127.0.0.1:19669/api/v1/diagnostics/queries \
  -H "X-ByoriDB-Session-Id: <session-id>"
```

Example shape:

```json
{
  "count": 1,
  "queries": [
    {
      "id": 42,
      "query_type": "match",
      "query_length_bytes": 25,
      "space": "example",
      "started_at_ms": 1785313593000
    }
  ]
}
```

The response intentionally omits raw query text and session IDs. A missing,
malformed, unknown, or expired session header returns HTTP 401; a valid
non-admin session returns HTTP 403. Diagnostic state is in process and is lost
on restart.

## Logging

The server initializes the default text `tracing_subscriber` formatter. Control
filtering with `RUST_LOG`:

```bash
export RUST_LOG='info,byoridb_graph=debug,byoridb_storage=info'
byoridb-server
```

Send stdout/stderr to the platform's log collector. The application does not
currently implement a `[logging]` file section, log-file rotation, JSON-format
toggle, or native Jaeger/OpenTelemetry configuration.

Important operational events include startup mode, redb path/cache/durability,
authentication outcome or bounded error type, query latency, slow/full-scan
warnings, readiness shutdown, drain timeout, and final checkpoint status. Raw
queries, usernames, password values, and session credentials are not included
by the current application logging paths described here.

## Kubernetes probes

The checked-in StatefulSet uses the actual endpoints:

```yaml
startupProbe:
  httpGet: { path: /health, port: 19669 }

readinessProbe:
  httpGet: { path: /ready, port: 19669 }

livenessProbe:
  httpGet: { path: /health, port: 19669 }
```

The repository manifest deliberately uses generous startup and termination
budgets for large redb files. Recalculate them from restore/startup drills
before tightening them. Do not use nonexistent `/health/live` or
`/health/ready` routes.

## Suggested operator checks

- alert on scrape and readiness loss;
- track query errors, latency distributions, slow queries, and sustained
  in-flight work;
- collect disk/PVC usage and process CPU/memory from the platform because the
  application storage gauge is not wired;
- watch redb checkpoint messages during planned shutdowns;
- exercise representative authenticated reads and `AS OF` queries after a
  restore or upgrade;
- treat a session or role change as local to the current process until
  multi-replica session semantics are implemented.

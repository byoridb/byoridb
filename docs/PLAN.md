# ByoriDB plan

> **English** | [한국어](PLAN.ko.md)
>
> Last code verification: 2026-07-29, against `76e7a79` plus the authentication
> and authorization hardening in this change set. This document records code and
> repository state, not the live state of any Kubernetes cluster.

This is the canonical project plan. It replaces historical snapshots that were
previously spread across roadmap and remediation documents. Implementation and
tests take precedence if this file ever falls behind.

## Status legend

- **Supported**: implemented on `main` or in the current change set and covered
  by automated tests.
- **Partial**: useful components exist, but the end-to-end product surface is
  incomplete or unsuitable for production.
- **Planned**: no supported implementation yet.
- **Constraint**: intentional or known limitation that callers must design for.

## Product direction

ByoriDB is a Rust semantic graph database core. It keeps a labelled-property
graph storage and query model, then adds ontology reasoning, temporal history,
provenance, and explanation as database primitives.

The core is deliberately not an operational-modelling application platform:

| ByoriDB core | Application or Studio layer |
|---|---|
| Classes, graph schemas, inference rules | Data-source mapping and ETL |
| Provenance and `WHY` explanation | Object/action/function UX |
| Constraint and shape evaluation | Workflow and write-back orchestration |
| Current and historical asserted facts | Business audit and approval flows |
| Graph traversal and recommendation | Domain-specific user experiences |

The near-term goal is correctness and a secure single-node deployment. A real
multi-node launcher remains a roadmap item.

## Verified baseline

| Area | Current state |
|---|---|
| Toolchain | Rust 1.90, edition 2021, protobuf compiler required for gRPC code generation |
| Package metadata | Workspace root reports `0.3.3`; deployments are identified by commit SHA rather than a maintained semver release line |
| Server | One `byoridb-server` process exposes gRPC and HTTP over a shared `GraphService` and redb store |
| Default listeners | gRPC `0.0.0.0:9669`, HTTP `0.0.0.0:19669`; neither listener provides native TLS |
| Storage | redb current view plus a separate history table in the same database |
| Durability | Immediate/fsync by default; relaxed durability is an explicit bulk-load trade-off |
| Authentication | Argon2 password hashes, random positive session identifiers, 24-hour default session TTL |
| Test gate | `cargo test --workspace --all-features -- --test-threads=1` |
| Static gates | `cargo fmt --all -- --check` and workspace Clippy with `-D warnings` |

The full test, formatting, and Clippy gates above passed on 2026-07-29. Avoid
hard-coding a test count: it changes whenever a regression test is added.

## Supported capabilities

### Graph and query execution

- Space, tag, edge, class, index, shape, and user-management statements.
- Vertex insert/update/delete/fetch/lookup and edge insert/delete/fetch/traversal.
- `MATCH`, `GO`, `FIND PATH`, compound statements, `EXPLAIN`, and `PROFILE`.
- Hash, modulo, and range partition metadata and distributed query components.
- Tag and edge indexes, reverse-edge indexes, tag-to-VID indexes, and persisted
  HNSW indexes for embedding recommendations.
- Query scan, traversal, path-count, and estimated result-memory limits.

The parser is nGQL-compatible in selected areas; it is not a complete
NebulaGraph nGQL implementation. Documented examples must be backed by parser or
end-to-end tests.

### Semantic graph layer

- Class hierarchies with multiple parents, disjointness, and equivalent classes.
- Subproperty, inverse, symmetric, transitive, equivalent-property, domain,
  range, and two-link property-chain semantics.
- Write-time forward materialization over the current view.
- Provenance records, `WHY` explanations, and DRed-style retraction for
  supported inferred facts.
- `owl:sameAs`-style canonical merging through the reserved `sameAs` edge.
- SHACL-inspired required, datatype, and predicate constraints.
- Structural similarity, embedding similarity, persisted HNSW search, and
  blended semantic/graph recommendation.

`sameAs` merging is intentionally irreversible today. Deleting a merged vertex,
the reserved edge type, or a `sameAs` assertion is rejected.

### Temporal history

Asserted vertex and edge mutations update the current view and append history in
one `batch_apply` transaction. Transaction timestamps are monotonically
allocated to avoid same-millisecond key collisions. redb and the memory backend
provide seek-based point-in-time resolution.

The public query surface currently supports:

```ngql
FETCH PROP ON person 42 AS OF 1780000000000;
FETCH PROP ON follows 1->2 AS OF 1780000000000;
FETCH PROP ON * 1->2 AS OF 1780000000000;
```

For this surface, the single `AS OF` value is applied to both valid and
transaction time. Writes use the engine-assigned current time for both axes.

### Security boundary

The current single-process security model includes:

- A non-empty `BYORIDB_ROOT_PASSWORD` requirement in the standalone server.
- Generic authentication failures and dummy password verification for unknown
  users to reduce account enumeration signals.
- Blank-password rejection, including legacy records containing an empty
  password hash.
- Recursive authorization of compound statements and mutating `PROFILE`
  statements.
- GOD/ADMIN-only user and role mutation, balance operations, and sensitive
  session/user listings.
- Session invalidation after local password, role, enablement, or user changes.
- Shared HTTP/gRPC authentication state and race-safe durable-user cache sync.
- Redaction of password-bearing queries and bearer session IDs from diagnostics,
  logs, and invalid-session responses.
- Administrator authentication for the active-query diagnostics endpoint.

See [SECURITY.md](../SECURITY.md) for the supported reporting channel and the
deployment controls that remain the operator's responsibility.

## Known constraints

### Security and tenancy

1. **No native transport encryption.** Passwords and bearer sessions are exposed
   to interception on an untrusted network. Use a private network and a trusted
   TLS/mTLS proxy until native TLS exists.
2. **No effective login rate limit.** Failed-attempt counters exist, but a correct
   password is not blocked and parallel Argon2 verification can consume async
   workers and CPU. Apply an external rate limit for exposed deployments.
3. **No space-scoped grants.** Built-in role permissions use `space="*"`, and
   public `GRANT`/`REVOKE` syntax assigns roles rather than per-space ACLs.
4. **Broad `Write` semantics.** `INSERT`, `UPDATE`, and `DELETE` currently map to
   `Write`; `ALTER` maps to `Create`. The `Delete` and `Alter` permission variants
   are not separate enforcement points for ordinary statements.
5. **Process-local auth state.** User and session caches are not coordinated
   across server processes. Immediate cross-node revocation is therefore not
   guaranteed.
6. **Public operational endpoints.** `/metrics` and `/api/v1/metrics` do not
   require authentication. The HTTP sign-out route carries a bearer-like session
   ID in the URL path; access logs must redact it.

### Distribution

Raft, snapshots, membership types, partition routing, storage RPCs, failure
detection, and distributed query helpers exist and have unit/integration tests.
The server can start a Meta gRPC listener when peers are configured.

This is still **partial**, not a supported multi-node deployment:

- Storage/Raft peer bootstrap is not wired end to end.
- Docker Compose services do not receive cluster configuration and are separate
  single-node servers.
- AKS manifests do not establish a tested multi-node topology.
- There is no production multi-node failover or partition-movement E2E gate.
- Session and authorization state is not cluster-wide.

### Temporal

- `VALID FROM`, `VALID TO`, `BETWEEN`, temporal `MATCH`, and temporal `GO` are not
  implemented.
- Historical inferred facts are not reconstructed; inference reads and writes
  only the current view.
- There is no retention, garbage collection, or user-facing history-list API.
- A monotonically allocated transaction timestamp can briefly move ahead of the
  wall clock under more than one write per millisecond.

### Query and data model

- `FIXED_STRING` is accepted in space DDL, but DML execution still requires
  integer VIDs. Until the type is implemented end to end, use `INT64`.
- `SHOW USER` currently returns a root-only placeholder. `SHOW SESSIONS` lists
  active users and selected spaces without bearer session IDs. The public
  parser accepts neither `SHOW USERS` nor `SHOW ROLES`.
- `UPDATE EDGE` is parser-only and has no working execution path. Edge
  `LOOKUP` is rejected; range predicates do not use an index.
- Geography encoding exists, but WKT/WKB decoding is not implemented.
- Many query paths still materialize intermediate rows rather than pulling a
  stream through physical operators.
- Reverse-edge data must be rebuilt for data created before the reverse index
  was introduced.

### Operations

- The repository contains deployment manifests and workflows, but this document
  does not assert that a live cluster is healthy or even present.
- Prometheus output and structured logs exist; maintained Grafana dashboards,
  alert rules, and a log-shipping stack do not.
- Backups are full redb snapshots. There is no incremental backup, WAL archive,
  object-store upload, or point-in-time restore workflow.

## Prioritized roadmap

### P0 — production security boundary

| ID | Work | Exit criteria |
|---|---|---|
| SEC-1 | Native TLS/mTLS or a documented, tested TLS proxy profile | Credentials never cross an untrusted network in plaintext; rotation is documented and tested |
| SEC-2 | Authentication load control | Per-source/account throttling, bounded Argon2 concurrency, and blocking work moved off async workers |
| SEC-3 | Space-scoped authorization | Grant syntax, durable ACL model, enforcement tests for every statement and compound/profile nesting |
| SEC-4 | Cluster-wide identity/session strategy | Revocation and authorization changes are consistent across supported server processes |
| SEC-5 | Operational endpoint hardening | Metrics access policy and bearer-free sign-out API are explicit and tested |

### P0 — correctness and operability

| ID | Work | Exit criteria |
|---|---|---|
| COR-1 | Resolve the `FIXED_STRING` VID mismatch | Either complete string-VID support across parser/plan/codec/keys or reject it at DDL |
| COR-2 | Complete identity metadata surfaces | Supported user, role, and session listing syntax reports durable, authorized state without secrets |
| COR-3 | Release/archive completeness | Binary archives include `LICENSE` and `NOTICES` and supported platforms are explicit |
| OPS-1 | Recovery validation | Automated restore test includes both current and history tables and records recovery time |

### P1 — semantic depth

| ID | Work | Notes |
|---|---|---|
| SEM-1 | Functional and inverse-functional properties | Must integrate safely with canonical merging |
| SEM-2 | General property chains | Extend the current two-link implementation with bounded planning and provenance |
| SEM-3 | Complete explanations | Include inferred vertex-type/domain/range paths consistently |
| SEM-4 | Stronger shapes | Edge cardinality and optional closed-world constraints |
| SEM-5 | Distributed materialization | Deferred until the multi-node runtime is supported |

### P1 — temporal query surface

| ID | Work | Notes |
|---|---|---|
| TMP-1 | Explicit valid-time writes | `VALID FROM`/`VALID TO` with independent transaction time |
| TMP-2 | Temporal traversal and patterns | `MATCH`, `GO`, and range semantics over history |
| TMP-3 | History inspection | Version-list API and `BETWEEN` query surface |
| TMP-4 | Lifecycle policy | Retention, compaction/GC, backup policy, and operational metrics |
| TMP-5 | Derived history research | Define whether and how historical inferred facts can be reproduced |

### P1 — supported distribution

| ID | Work | Exit criteria |
|---|---|---|
| DIST-1 | Storage/Raft bootstrap | Nodes discover peers, form groups, and recover membership without manual state edits |
| DIST-2 | Deployment wiring | Compose/Kubernetes configure one coherent cluster rather than independent servers |
| DIST-3 | Failure E2E | Multi-node write/read, leader loss, snapshot restore, membership change, and rolling restart tests |
| DIST-4 | Distributed query completion | Partition pruning, aggregation, joins, sorting, and bounded partial-result semantics |

### P2 — measured performance and execution architecture

- Add indexed range scans for `LOOKUP` ([issue #1](https://github.com/byoridb/byoridb/issues/1)).
- Harden multi-ID fetch and projection at large scale
  ([issue #10](https://github.com/byoridb/byoridb/issues/10)).
- Add parallel range scans and partial aggregation only after a reproducible
  workload shows the single-stream scan is the bottleneck.
- Reorder `MATCH` patterns by selectivity without changing variable semantics.
- Move toward pull-based physical operators as described in
  [VOLCANO_MIGRATION_PLAN.md](VOLCANO_MIGRATION_PLAN.md).
- Add lightweight edge-destination and weighted-property decoding where
  profiling demonstrates wire or decode cost.

### P2 — API and observability

- Promote complex values from JSON fallback to first-class protobuf messages.
- Publish dashboards, alert rules, and a supported log-shipping example.
- Add compatibility tests for HTTP decimal-string session IDs and client
  language precision.
- Define a stable release/versioning policy before advertising semver
  compatibility guarantees.

## Regression-sensitive areas

### Query-correctness H series

H-1 through H-6 are fixed and must remain covered:

| ID | Regression |
|---|---|
| H-1 | Distinct, persistent space IDs in `SHOW SPACES` |
| H-2 | No cross-space tag/edge metadata leakage |
| H-3 | `GO` returns the real destination rather than VID zero |
| H-4 | `LOOKUP` decodes protobuf vertices without VID zero fallback |
| H-5 | Edge `FETCH PROP` resolves an edge rather than a vertex |
| H-6 | Comma-separated `MATCH` patterns preserve all clauses and bindings |

Run the `test_h1_*` through `test_h5_*` tests and
`match_impl::h6_multipattern_tests` after changing the related parser, schema,
key, plan, or executor paths.

### Temporal paths

Changes to the KV history table, DML, temporal executor, parser, or plan must
verify both history and current-view behavior:

```bash
cargo test -p byoridb-kvstore --test temporal -- --test-threads=1
cargo test -p byoridb-parser fetch_as_of -- --test-threads=1
cargo test -p byoridb-executor fetch_as_of -- --test-threads=1
cargo test --workspace --all-features -- --test-threads=1
```

### Raft paths

The implementation under `byoridb-storage/src/raft/` is custom and has no
external compatibility certification. Any change requires the distributed E2E
test in addition to the full workspace gate.

### Authentication and authorization paths

Changes to graph auth, sessions, HTTP/gRPC wiring, or user execution must cover:

- guest denial for direct, compound, and profiled mutations;
- administrator-only user/session operations;
- durable user hydration and root-record isolation;
- local session invalidation after role/password/user changes;
- password and session-ID redaction;
- invalid and blank credential behavior;
- concurrent authentication, revocation, diagnostics, and sign-out.

The focused integration suite is `tests/security_authz_test.rs`.

## Completed milestones

| Date | Milestone |
|---|---|
| 2026-06 | Reverse indexes, variable-length paths, class hierarchy, semantic edge rules, materialization, consistency, `is_a`, canonical `sameAs`, provenance/`WHY`, DRed retraction, and two-link property chains |
| 2026-06 | Structural, embedding, HNSW, and blended recommendation |
| 2026-07-01 | Shape validation and equivalent class/property support |
| 2026-07-10 | Asserted vertex/edge temporal history and vertex `AS OF` |
| 2026-07-13 | Atomic current/history writes, monotonic transaction time, seek-based resolution, and temporal E2E tests |
| 2026-07-14 | Edge and wildcard-edge `AS OF` reads, including negative integer VIDs |
| 2026-07-29 change set | Recursive RBAC, durable auth/cache reconciliation, session revocation, credential redaction, shared HTTP/gRPC auth state, and root-secret hardening |

## Decision rules

1. Fix a demonstrated security or correctness defect before adding surface area.
2. Do not call distribution supported until deployment wiring and failure E2E
   tests exist.
3. Preserve the current-view fast path when extending temporal history.
4. Add semantic rules only with provenance and deletion/retraction behavior
   defined up front.
5. Require a measured workload before large execution-engine optimizations.
6. Keep application workflow, data-source integration, and write-back logic out
   of the database core.
7. Identify deployed artifacts by commit SHA until a maintained release line is
   established; do not invent version numbers in documentation.

## Standard verification

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features -- --test-threads=1
cargo build --workspace --release
```

Documentation-only changes should additionally build both mdBooks and verify
that English and Korean source trees contain the same page paths.

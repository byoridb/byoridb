# ByoriDB plan

> **English** | [한국어](PLAN.ko.md)
>
> Last code verification: 2026-08-08, against `main` at `9200800` with PR #65.
> This document records code and repository state, not the live state of any
> Kubernetes cluster.

This is the canonical project plan. It replaces historical snapshots that were
previously spread across roadmap and remediation documents. Implementation and
tests take precedence if this file ever falls behind.

## Status legend

- **Supported**: implemented on current `main` and covered by automated tests.
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
| Package metadata | `[workspace.package]` in the root `Cargo.toml` is the one version all ten crates inherit and every binary reports. `.github/workflows/autorelease.yml` bumps its patch on every push to `main`, commits that to `main`, and publishes the matching tag, so a released binary's `--version` and its tag always agree. Each binary also carries the commit SHA it was built from, which identifies the build more precisely than the tag does |
| Server | One `byoridb-server` process exposes gRPC and HTTP over a shared `GraphService` and redb store |
| Default listeners | gRPC `0.0.0.0:9669`, HTTP `0.0.0.0:19669`; neither listener provides native TLS |
| Storage | redb current view plus a separate history table in the same database |
| Durability | Immediate/fsync by default; relaxed durability is an explicit bulk-load trade-off |
| Authentication | Argon2 password hashes, random positive session identifiers, sliding session TTL defaulting to 24 hours and settable via `auth.session_ttl_secs` |
| Protocol/config stack | tonic, tonic-prost, and prost 0.14; config 0.15.25 |
| Test gate | `cargo test --locked --workspace --all-features -- --test-threads=1` |
| Static gates | `cargo fmt --all -- --check` and locked workspace Clippy with `-D warnings` |

The full test, formatting, Clippy, dependency-audit, and release-build gates
above passed on 2026-08-08. Avoid hard-coding a test count: it changes whenever
a regression test is added.

## Audited GitHub issue status

Code state and GitHub issue state are tracked separately. The issue state below
was checked on 2026-08-17; a code-complete branch does not imply that its issue
is closed:

| Issue | Code status | Evidence or remaining work |
|---|---|---|
| [#26](https://github.com/byoridb/byoridb/issues/26) | Resolved by PR #32; issue closed | Atomic current/history writes, monotonic transaction timestamps, seek-based `AS OF`, and E2E coverage landed in [PR #32](https://github.com/byoridb/byoridb/pull/32). |
| [#27](https://github.com/byoridb/byoridb/issues/27) | Resolved by PR #30; issue closed | Workspace-wide Clippy with `-D warnings` is enforced by CI; the change landed with the core/product split in [PR #30](https://github.com/byoridb/byoridb/pull/30). |
| [#29](https://github.com/byoridb/byoridb/issues/29) | Resolved by PR #35; issue closed | Dependabot now targets `main`, and follow-up dependency PRs have opened and merged; the configuration fix landed in [PR #35](https://github.com/byoridb/byoridb/pull/35). |
| [#25](https://github.com/byoridb/byoridb/issues/25) | Resolved in `byori`; tracking issue closed | Agent-memory ownership moved to [byoridb/byori](https://github.com/byoridb/byori); typed-wiki schema bootstrap/versioned migration landed in [3ce7730](https://github.com/byoridb/byori/commit/3ce7730e19f04bd980b10829c9c6e34b9ecf86d4), followed by guarded structured-memory tools in [18d605f](https://github.com/byoridb/byori/commit/18d605f214a8fb8ce6476b9eccdc9efc9162b714). |
| [#23](https://github.com/byoridb/byoridb/issues/23) | Engine half resolved; issue open for the client half | The server maps missing or expired HTTP sessions to `401 SESSION_EXPIRED` ([PR #35](https://github.com/byoridb/byoridb/pull/35)), and an authorization denial is no longer carried by the same status: it is `403 PERMISSION_DENIED` ([PR #85](https://github.com/byoridb/byoridb/pull/85)), so `401` means exactly "a new session will help". v0.4.0 ships both. `byori`'s MCP client already classifies on status and keeps `400`-plus-`session` only as a deliberate fallback for pre-0.4.0 engines. Remaining, tracked as [byori#54](https://github.com/byoridb/byori/issues/54): it has no `429`/`Retry-After` branch, so a throttled or locked login escapes `_raw_query` as a raw `HTTPError` and burns `_ensure_ready`'s 30×2s retry budget on a window it cannot outlast. An integration test over all four classes is still required. |
| [#24](https://github.com/byoridb/byoridb/issues/24) | Narrowed to Windows; issue open | Tagged releases build Linux x86_64/arm64 and macOS x86_64/arm64 archives, and every macOS binary is Developer ID signed under the hardened runtime and notarized, with the release failing on anything other than `Accepted`. The ticket is not stapled, because `stapler` cannot write into a bare executable, so Gatekeeper resolves it online; v0.3.3 and earlier archives are not signed retroactively. The issue was rescoped to what is left: Windows is neither built nor documented as unsupported, and the server's shutdown handler is a `#[cfg(not(unix))]` no-op, so adding the target alone would not give it graceful shutdown. One gap moved out of this repo rather than closing — `byori/install.sh` maps only `Linux/x86_64`, so the ARM Linux archive the engine now publishes is not installable; recorded on the issue. |
| [#28](https://github.com/byoridb/byoridb/issues/28) | Resolved by PR #51; issue closed | The release workflow packages and verifies `LICENSE` and `NOTICES.md` at the root of every new tagged archive. Published v0.3.3 and older archives are not changed retroactively. |
| [#49](https://github.com/byoridb/byoridb/issues/49) | Resolved by PR #57; issue closed | Standalone `FIXED_STRING` uses persistent negative `i64` surrogates while preserving storage-key and RPC/protobuf integer contracts. Unknown reads remain misses, collision claims avoid live graph occupancy, legacy non-negative records have a write-frozen read/delete bridge, and distributed mapping plus `RECOMMEND` remain explicitly unsupported. |
| [#1](https://github.com/byoridb/byoridb/issues/1) | Resolved by PR #54; issue closed | Local single-field Boolean and integer `LOOKUP` comparisons use bounded index range scans with stale-entry revalidation and correct `OFFSET`/`LIMIT`; unsupported key domains retain the predicate-scan fallback, and distributed ordered ranges fail closed. |
| [#77](https://github.com/byoridb/byoridb/issues/77) | Resolved in code; issue open pending review | `UPDATE EDGE` now has an execution path. The planner used to drop `update_type`, `dst_vid`, `edge_name`, and `ranking`, which turned the statement into a vertex update with no tag and produced "Tag name required for UPDATE" — an error naming a concept the statement never mentions. `UpdatePlan` carries an `EdgeUpdateTarget`, and `execute_update_edge` merges properties, rewrites the forward and reverse current-view keys plus a history version in one `batch_apply`, and moves edge index entries off the old values. Deliberately **not** an upsert, unlike vertex UPDATE: a missing edge is a no-op, because creating one here would have to maintain the degree counters and ontology triples `INSERT EDGE` asserts, and skipping either would corrupt a precomputed `COUNT` or leave inference stale. Source, destination, type, and rank are not assignable, so an update never moves an edge and those counters are unchanged by construction. |
| [#80](https://github.com/byoridb/byoridb/issues/80) | Resolved by PR #103; issue closed | `auth.session_ttl_secs` / `BYORIDB__AUTH__SESSION_TTL_SECS` sets the bearer session TTL, defaulting to the previous hardcoded 24 hours; zero and values beyond one year are rejected at startup. It lives under `[auth]` rather than the `[server]` key the issue proposed, so it sits with the other authentication settings added by #76; the proposed name was never released, so nothing depended on it. The issue's secondary finding was real: `GraphService::with_auth` built its graph session store with an independent 24h default, and because the two stores are reconciled on access with the shorter lifetime winning, a configured TTL above 24h would have been silently capped. `with_auth` now derives the graph store's TTL from the bearer's, with `with_session_ttl` retained only to construct divergence deliberately in tests. |
| [#76](https://github.com/byoridb/byoridb/issues/76) | Resolved by PR #93; issue closed; released in v0.4.1 | The login window, per-account and per-source failure budgets, Argon2 concurrency bound, lockout threshold, and lockout duration are settable under `[auth]` in `byoridb.toml` and `BYORIDB__AUTH__*`, with defaults unchanged. The issue's proposal listed only the first four, which would not have addressed its own problem statement — the lockout is what locks a single-user deployment out on a mistyped secret — so `lockout_duration_secs = 0` disables it while the window budgets keep bounding guessing. Startup rejects values that would refuse every login rather than clamping them. Nothing relaxes automatically from the bind address. |
| [#90](https://github.com/byoridb/byoridb/issues/90) | Resolved by PR #91; issue closed | The login throttle now counts only failures, so concurrent logins with a valid credential all succeed and queue on the Argon2 pool instead of being refused. The reported "lockout" was the 60 s per-account window (20 attempts), not the 300 s lockout, and it was reached because successes were counted. A refusal that never evaluated a credential — spent budget or lockout — is `429 TOO_MANY_ATTEMPTS` with `Retry-After` (gRPC `error_code` 3) and is not itself counted; evaluated failures stay collapsed into `401 AUTH_FAILED`. Reporting a lockout distinctly discloses account existence to a caller that spends five wrong guesses, which is the tradeoff the issue asked for. Throttle policy is documented in `book/src/reference/api.md`; making it configurable stays with #76. |
| [#10](https://github.com/byoridb/byoridb/issues/10) | Engine scope resolved in code; issue open for harness acceptance | Multi-ID `FETCH` is covered at 1,000 VIDs, destination projections use one deduplicated `batch_get`, and `EXPLAIN`/`PROFILE` expose the batch as `GetVertices`; the separate LDBC harness still needs the Q9 conversion and `<10s` measurement. |

Close a resolved-in-code issue only after recording its implementation and
verification evidence. Keep partial and unresolved issues open until their
remaining acceptance criteria pass.

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

- A non-blank `BYORIDB_ROOT_PASSWORD` requirement in the standalone server.
- Generic authentication failures and valid dummy Argon2 verification for
  unknown, blank, disabled, or locked users to reduce account enumeration
  signals.
- Rejection of malformed durable-user records with a blank username or an
  unsupported, incomplete, or out-of-policy Argon2id password hash before any
  authentication-cache mutation.
- Recursive authorization of compound statements and mutating `PROFILE`
  statements.
- GOD/ADMIN-only user and role mutation, balance operations, and sensitive
  session/user listings.
- Session invalidation after local password, role, enablement, or user changes.
- Shared HTTP/gRPC authentication state and race-safe durable-user cache sync.
- Payload-free parser diagnostics for identifier, string, and numeric tokens,
  including malformed credential statements split by comments, plus redaction
  of password-bearing queries and bearer session IDs from diagnostics, logs,
  and invalid-session responses.
- Fallible entropy handling for bootstrap credentials, Argon2 salts, and
  authentication session identifiers instead of startup or login panics.
- Process-local 60-second login windows capped at 20 attempts per account and
  60 per peer IP, with at most four concurrent Argon2 verifications executed
  on Tokio's blocking pool.
- Administrator authentication for the active-query diagnostics endpoint.

See [SECURITY.md](../SECURITY.md) for the supported reporting channel and the
deployment controls that remain the operator's responsibility.

## Known constraints

### Security and tenancy

1. **No native transport encryption.** Passwords and bearer sessions are exposed
   to interception on an untrusted network. Use a private network and a trusted
   TLS/mTLS proxy until native TLS exists.
2. **Process-local login limits.** Account and direct-peer-IP windows plus an
   Argon2 concurrency bound protect one process, but counters are not shared
   across replicas and a proxy's address is treated as the source. Keep an
   external trusted-proxy/cluster-wide limit for exposed deployments.
3. **No space-scoped grants.** Built-in role permissions use `space="*"`, and
   public `GRANT`/`REVOKE` syntax assigns roles rather than per-space ACLs.
4. **Broad `Write` semantics.** `INSERT`, `UPDATE`, and `DELETE` currently map to
   `Write`; `ALTER` maps to `Create`. The `Delete` and `Alter` permission variants
   are not separate enforcement points for ordinary statements.
5. **Process-local auth state.** User and session caches are not coordinated
   across server processes. Immediate cross-node revocation is therefore not
   guaranteed.
6. **Public operational endpoints.** `/metrics` and `/api/v1/metrics` do not
   require authentication. HTTP sign-out and query diagnostics carry the
   bearer-like session ID in `X-ByoriDB-Session-Id`; every external logging
   layer must redact that header.

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

- `FIXED_STRING(N)` VIDs are supported in standalone graph CRUD, FETCH, GO,
  FIND, LOOKUP, and MATCH. New durable per-space mappings use the negative
  `i64` namespace so existing codec, key, storage, partition, and RPC contracts
  remain unchanged and a write-frozen live non-negative legacy bridge stays
  disjoint. Cluster-wide mapping ownership/replication is not implemented, and
  `RECOMMEND` remains INT64-only.
- `SHOW USER`/`SHOW USERS` and `SHOW ROLE`/`SHOW ROLES` list the built-in root
  and persisted users with their roles. `SHOW SESSIONS` lists active users and
  selected spaces without bearer session IDs.
- `UPDATE EDGE` updates an existing edge's properties, refreshing the reverse
  copy and edge indexes in step; it is not an upsert, so a missing edge is a
  no-op rather than a creation. Edge `LOOKUP` is rejected. Local single-field
  Boolean and integer ordered predicates can use a tag index; strings,
  floating-point key domains, compound ranges, and distributed range execution
  are not index-enabled.
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
| SEC-2 | ✅ Authentication load control | Per-account/direct-peer throttling, bounded Argon2 verification concurrency, and login verification moved to the blocking pool |
| SEC-3 | Space-scoped authorization | Grant syntax, durable ACL model, enforcement tests for every statement and compound/profile nesting |
| SEC-4 | Cluster-wide identity/session strategy | Revocation and authorization changes are consistent across supported server processes |
| SEC-5 | Operational endpoint hardening | Metrics access policy and bearer-free sign-out API are explicit and tested |

### P0 — correctness and operability

| ID | Work | Exit criteria |
|---|---|---|
| COR-1 | ✅ Resolve the standalone `FIXED_STRING` VID mismatch | String VIDs round-trip across graph CRUD and traversal with negative surrogates while existing `i64` storage/RPC contracts remain compatible; distributed mapping and `RECOMMEND` are out of scope |
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

- Extend indexed `LOOKUP` range coverage beyond local single-field Boolean and
  integer predicates only after string/float key ordering and distributed
  execution have explicit correctness contracts.
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
cargo test --locked -p byoridb-kvstore --test temporal -- --test-threads=1
cargo test --locked -p byoridb-parser fetch_as_of -- --test-threads=1
cargo test --locked -p byoridb-executor fetch_as_of -- --test-threads=1
cargo test --locked --workspace --all-features -- --test-threads=1
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

The focused coverage is inline in the parser and graph crates; there is no
standalone `tests/security_authz_test.rs`. Run:

```bash
cargo test --locked -p byoridb-parser --lib
cargo test --locked -p byoridb-graph --lib -- --test-threads=1
```

## Completed milestones

| Date | Milestone |
|---|---|
| 2026-06 | Reverse indexes, variable-length paths, class hierarchy, semantic edge rules, materialization, consistency, `is_a`, canonical `sameAs`, provenance/`WHY`, DRed retraction, and two-link property chains |
| 2026-06 | Structural, embedding, HNSW, and blended recommendation |
| 2026-07-01 | Shape validation and equivalent class/property support |
| 2026-07-10 | Asserted vertex/edge temporal history and vertex `AS OF` |
| 2026-07-13 | Atomic current/history writes, monotonic transaction time, seek-based resolution, and temporal E2E tests |
| 2026-07-14 | Edge and wildcard-edge `AS OF` reads, including negative integer VIDs |
| 2026-07-30 | Recursive RBAC, durable auth/cache reconciliation, session revocation, credential redaction, shared HTTP/gRPC auth state, root-secret hardening, and the config/tonic dependency migrations |
| 2026-08-03 | Local bounded `LOOKUP` index range scans, Linux arm64 release artifacts, and standalone `FIXED_STRING` VID support on PR #57 |
| 2026-08-08 | Non-blank credential enforcement, validated persisted Argon2id profiles, fallible authentication entropy, and payload-free parser diagnostics on PR #65 |
| 2026-08-08 | Process-local account/IP login throttling and bounded blocking-pool Argon2 verification |

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
cargo check --locked --workspace --all-targets --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features -- --test-threads=1
cargo build --locked --workspace --release
```

Documentation-only changes should additionally build both mdBooks and verify
that English and Korean source trees contain the same page paths.

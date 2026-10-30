# ByoriDB development plan

> **English** | [한국어](PLAN.ko.md)
>
> Updated: 2026-10-30. Code baseline: `main` at `6d77d89` (version **v0.4.21**).
> This is a source-based plan, not a live deployment report. Unreleased work and
> checks performed during this revision are identified separately below.

This is the authoritative delivery plan for an **agent-first, general-purpose
graph and ontology database**. Agents are its direct consumers; people operate,
evaluate, govern, and purchase deployments. Agent memory is one use case, not the
scope of the engine. The [product philosophy and design](PRODUCT_DESIGN.md) define
the target contracts; this plan tracks their implementation and acceptance. The
[book roadmap](../book/src/development/roadmap.md) is the public summary.
Update both languages together. Earlier rationale, retired task IDs, and incident
records remain in the [archived plan](archive/PLAN.pre-readiness-2026-10-30.md).

## Product direction

Keep the Rust labelled-property graph core and deepen its semantic layer:
classes, rules, provenance, explanations, constraints, and historical facts.
Design the primary interface around an agent discovering authorized capabilities,
submitting typed operations, inspecting bounded results and evidence, and safely
retrying or resolving conflicts. Query correctness, recovery, and authorization
remain prerequisites. Evaluate successful work and correct final state before
call count, tokens, or latency; no agent-efficiency advantage is established yet.

The database owns storage, queries, inference, explanations, constraint evaluation,
and protocol contracts. Applications and orchestration layers own goals, model
selection, business actions, workflows, approvals, and external write-back.
Protocol adapters expose the same database semantics; they do not grant authority
or make an unsupported engine capability available. A separate MCP server may
belong to `byori`; this plan does not claim one is implemented here.
Logical inference evidence and source/audit lineage are distinct contracts.
An explanation demonstrates derivation from recorded facts and rules, not the
truth of an external source. Selected nGQL and RDFS/OWL-style features do not
imply complete nGQL, OWL, SHACL, RDF, or SPARQL conformance.

The product direction is approved; the new machine contracts below are
**PLANNED or IN PROGRESS** except the verified slices recorded below. The
following table summarizes the pre-foundations baseline. Do not publish proposed fields, tool names, idempotency,
revision checks, or snapshot guarantees as available API behavior.

## Code baseline and boundaries

| Area | Available at the code baseline | Remaining boundary |
|---|---|---|
| Runtime | Rust 1.90; a single server shares a graph service and redb store across HTTP/gRPC | Raft/routing components exist; a supported multi-node runtime and failover gate do not |
| Queries | Vertex/edge CRUD, `FETCH`, `GO`, `MATCH`, tag/edge `LOOKUP`, path finding, `EXPLAIN`/`PROFILE` | Resource guards lack a uniform complete-or-error contract; compound statements are not transactions |
| Interfaces | HTTP/gRPC query execution with `read_only` enforcement; edge `WHY` returns string-valued columns | Typed query-parameter execution, structured proof/capability envelopes and the proposed agent mutation contracts are not implemented |
| Recent fixes | Node labels support AND/OR; edge type alternation; edge `FETCH @rank`; trailing input and unknown MATCH functions rejected | Preserve these regressions; omitted edge FETCH rank still selects all ranks |
| Indexes | Tag/edge equality and eligible local ordered ranges, reverse/tag-to-VID indexes, atomic edge/index update batches | Unsupported range domains fall back to scans; distributed ordered ranges are unsupported |
| Vertex IDs | INT64 and standalone `FIXED_STRING(N)` graph CRUD/traversal | `WHY` syntax and `RECOMMEND` require integer IDs; distributed string mapping is unsupported; changing VID type requires a new space |
| Ontology | Class hierarchy, disjoint/equivalent classes, semantic edge rules, two-link chains, current-view inference, edge `WHY`, DRed, shapes | Caps can leave partial closure; vertex-type explanation and shape/inference policy need completion; `sameAs` merge is irreversible |
| Recommendation | Structural, embedding, persisted HNSW, and blended recommendation | String VID parity and workload acceptance remain separate work |
| Time | Atomic asserted current/history writes, monotonic transaction time, vertex/edge `FETCH ... AS OF` | No temporal `MATCH`/`GO`, explicit valid-time intervals, history-list API, retention/GC, or historical inference reconstruction |
| Recovery | Full snapshot backup/restore preserves current and history tables | CLI verification does not yet prove content integrity or successful restoration; no incremental/PITR workflow |
| Security | Shared HTTP/gRPC auth, durable users, built-in roles, recursive authorization, local revocation, configurable login throttling/session TTL | No native TLS, space grants, or cluster-wide revocation; metrics exposure needs an explicit policy |

Storage range primitives accept Boolean, integer, and eligible floating-point
boundaries; not every query/type combination uses an index. String key encoding
is not lexical. Check the executor and `EXPLAIN` together when extending coverage.
Compose replicas currently run independent servers, not one shared-data cluster.

## Status and delivery order

**DONE** requires implementation, acceptance checks, and documentation evidence.
**IN PROGRESS** is unfinished branch work, not a released capability. **PLANNED**
has not passed acceptance. **GATED** requires the stated demand or design decision.
Dependencies use stable task IDs; stages express priority, not fixed dates.
Independent tests, contract design, and documentation may proceed in parallel.
A dependency on an agreed design is distinct from a dependency on completed
implementation; advertised capabilities require their implementation acceptance.
Security and correctness defects preempt feature work regardless of stage.

### Release prerequisite — dependency audit

| ID / status | Work | Acceptance |
|---|---|---|
| DEP-1 / DONE (unreleased) | Resolve the current dependency-audit failure in a separate compatibility-reviewed change | Investigate dependency paths and available fixes; update compatible dependencies/lockfile, pass the current audit and workspace checks, require the audit in the automatic release path, and document any unresolved advisory and release decision explicitly. |

On 2026-10-30, the baseline lockfile audit reported **10 vulnerabilities**
across `pulldown-cmark`, `rustls`, `rustls-webpki`, `zstd`, `zstd-safe`, and
`zstd-sys`. The foundations change updates compatible dependencies, including
tonic 0.14.9's patched zstd dependency, without disabling compression or ignoring
advisories. The current candidate audit reports **zero vulnerabilities**.
Gzip, zstd, and uncompressed requests/responses receive real gRPC round-trip tests.

The release workflow now audits its exact release ref after signing-secret
preflight and before building or publishing. Auto-release audits before changing
the version and each candidate before pushing, including after a rebase. These
checks close the earlier gap where only the separate CI release-build waited
for audit. The workflow changes are integrated into `main`; a local audit does
not prove a GitHub Actions run or release succeeded.

`bincode` 1.3.3's unmaintained warning remains a separate storage-format migration
constraint (#105), not an ignored vulnerability. DEP-1 passes its local acceptance: final workspace, compression and audit
checks are recorded below. Hosted CI and release execution must still be verified.

The preceding [CI run #342](https://github.com/byoridb/byoridb/actions/runs/41050549029)
failed on `79430a1` because Dependency Audit found those 10 vulnerabilities.
Check, Format, Clippy, Test and the Linux arm64 build passed; the gated release
build was skipped. The separate Auto-release run succeeded, confirming the
release-gate gap addressed by DEP-1. New runs must verify the patched lockfile.

### Stage 1 — correctness and machine contracts

| ID / status | Work | Dependencies | Acceptance |
|---|---|---|---|
| QRY-1 / DONE (unreleased) | Propagate `OPTIONAL MATCH` errors | None | Genuine no-match retains null-binding semantics. Errors returned by optional execution and pattern resolution propagate unchanged, with injected storage failures, partial-row failures and chained cases covered. Eight focused regressions and workspace/documentation checks pass; see the prior-slice verification record below. The next MATCH candidate/partition slice is tracked in QRY-2. DEP-1 remains a release-readiness requirement. |
| QRY-2 / IN PROGRESS | Uniform scan/traversal/result completeness | QRY-1; co-design API-2/AGT-2 result contract | Audit limiting paths and swallowed lower MATCH index/partition errors. Below/at/above a cap, return the complete query-defined result or an explicit error. User `LIMIT` is query semantics. Any partial mode needs separate design, explicit opt-in and a machine-readable reason; no partial mode is currently promised. |
| QRY-3 / PLANNED | Inference completeness and recovery | QRY-2 error contract | Cap/error cannot report completed closure. Fault tests expose asserted/derived write state; retry/rebuild converges without duplicate or stale provenance. Specify rollback versus resumable recovery first: an error alone does not prove rollback. |
| QRY-4 / PLANNED | Real server deadline and cancellation | QRY-2; QRY-3 for writes | Expensive execution stops at tested cancellation points, cleans up resources/diagnostics and returns stable HTTP/gRPC errors. Test write interruption and atomic boundaries; a setting or client disconnect alone is not proof of cancellation. |
| API-1 / PLANNED | Typed query parameters | Design now; QRY-1–2 for execution acceptance | Pass IDs, Unicode, nulls and lists without interpolation over HTTP/gRPC; consistent missing/type errors; distinguish parameters from compound variables. |
| API-2 / IN PROGRESS | Stable errors, values and capabilities | Minimum envelope design now; full acceptance needs QRY-1–4 and TXN-1 | Fix error/retry/outcome meanings, integer precision, complex values and version compatibility. Define the discovery envelope before AGT-1 implementation. Admit capabilities only after their runtime checks; unknown mutation outcome is not a safe-to-retry success/failure claim. |
| AGT-1 / IN PROGRESS | Structured operations and authorized schema/capability discovery | Agreed API-2 discovery/error/value envelope; API-1 for parameterized execution; existing auth; SEC-3 before scoped-grant claims | Start with read-only discovery and contract fixtures. Return only authorized schema and implemented capabilities with version/limits and permission context. Test missing/denied/unsupported states, read-only restrictions and no schema leakage. Add typed operations incrementally; text query escape hatches retain the same authorization and result rules. |
| AGT-2 / PLANNED | Bounded result and evidence envelope | API-2 design, QRY-2; QRY-3/4 and TXN-1 for respective claims | Distinguish query completeness, page completion and inference status; declare snapshot/revision support or its absence. Bound rows, bytes, evidence and execution; define continuation consistency and provenance scope. Truncation cannot become complete success; rejected budgets and unsupported modes are explicit. |
| EVAL-1 / IN PROGRESS | Reproducible agent workflow harness | Baseline fixtures now; version each capability as implemented | Discover, traverse, explain, retract, retry after timeout, race writes and deny access. Verify exact final state, evidence and denied side effects before cost. Pin model/version/date, prompts/settings, data/seed, repetitions and budgets; declare success targets before runs. Record unsupported baseline tasks; compare calls/tokens/latency only for equivalent successful work. |

QRY-1 is implemented and verified on `main` in commit `10a7811`; release
publication has not been verified here. The foundations commits through
`20d464b` are integrated into `main` and advance DEP-1, MATCH candidate/variable-length
completeness in QRY-2, discovery v1 in API-2/AGT-1, and deterministic EVAL-1 fixtures.
QRY-2 still needs LOOKUP/path, projection/decode-error and optimized count audits.
API-2 and AGT-1 remain IN PROGRESS: discovery does not implement typed query
parameters, general operation envelopes, or space-scoped grants. EVAL-1's
[fixture protocol](../tests/fixtures/agent-workflows/README.md) covers exact
answers, denied side effects and final state; no autonomous model run is claimed. QRY-3 failure/recovery design, OPS-1 restore fixtures and
PERF-1 baseline collection can also start without waiting for the entire stage.
Stage 1 is complete only when every listed task passes its acceptance; early
contract work does not mark later write, security, or cancellation behavior done.

### Stage 2 — recovery, reliable writes, and access boundaries

| ID / status | Work | Dependencies | Acceptance |
|---|---|---|---|
| OPS-1 / PLANNED | Backup content checks and restore drills | None | Detect missing/truncated/corrupt content. Restore in isolation; compare current/history queries, tombstones, schema, indexes, users and string-ID mappings; record recovery time. Do not damage the source or an existing target. |
| OPS-2 / PLANNED | Storage-format version and upgrade policy | OPS-1 | Record format independently of package version; test old fixtures, unsupported-version rejection, backup/recovery and data preservation; document the supported migration window. |
| TXN-1 / PLANNED | Compound partial-write contract | QRY-1; co-design API-2 mutation outcomes | Document/test committed prefixes after a failing statement, stable failed-statement/partial-application reporting and retry safety. Do not imply rollback. |
| TXN-2 / PLANNED | Atomic single-space DML batch | TXN-1, OPS-2, QRY-3 | Define supported statements/bounds; injected failure/crash leaves all or none of current data, history, indexes, counters and supported inference/provenance effects. Reject unsupported operations before mutation. |
| SEC-3 / PLANNED | Durable space-scoped grants | Co-design ACL/discovery errors with API-2/AGT-1; no SDK dependency | Deny cross-space access via direct, compound, profiled, discovery and administrative paths; test grant/revoke, restart and local session invalidation. Do not leak schema, evidence or operation outcomes through lookup endpoints. |
| SEC-1 / PLANNED | Supported TLS deployment profile | None; can start now | Test a trusted proxy or native TLS, private backend listeners, header/credential redaction and certificate rotation; no plaintext credential path over untrusted networks. |
| AGT-3 / PLANNED | Idempotency and durable mutation-outcome lookup | TXN-2, OPS-2, SEC-3; agreed API-2 mutation envelope | Atomically commit a scoped request key/fingerprint, mutation and durable outcome. Test concurrent duplicates, key/payload conflicts, crash/restart and timeout lookup. Define retention/expiry: unknown or expired outcomes must not be silently replayed as known-safe writes. Authorize lookup/replay and bound storage. |
| AGT-4 / PLANNED | Expected revisions, conflicts and preview/apply | TXN-2, AGT-3, SEC-3; agreed API-2 mutation envelope | Atomically validate expected revisions and apply supported changes. Preview binds request/affected state and expiry; apply rechecks authorization and relevant revisions. Reject stale/expired plans without side effects; preview is not an authorization grant. Test concurrent writes, revoked access and deterministic conflict reporting. |

SEC-1/3 move early because agents need bounded authority before shared writes;
they do not wait for SDKs. Configurable login throttling (SEC-2) is implemented.
Cluster-wide revocation (SEC-4) remains GATED with distribution. Until SEC-1/3
pass, follow the single-node boundary in [SECURITY.md](../SECURITY.md), including
network isolation.

Keep `bincode` 1.3.3 as the current format decision ([#105](https://github.com/byoridb/byoridb/issues/105)).
Its unmaintained advisory is separate from DEP-1's vulnerability findings. Replacing
it requires on-disk/Raft/snapshot/wire migration, not a dependency-only bump.
Decouple encoder-specific error types from `KVStoreError` when preparing that work.

### Stage 3 — ontology consistency and evidence

| ID / status | Work | Dependencies | Acceptance |
|---|---|---|---|
| SEM-6 / PLANNED | String VID parity for `WHY`/recommendation | QRY-1–3 | Equivalent INT64/string-ID fixtures give equivalent results with external IDs preserved; unknown reads do not allocate mappings. |
| SEM-3 / PLANNED | Explain inferred vertex types | QRY-3, SEM-6; AGT-2 evidence envelope | Domain/range and class derivations include rules/premises and remain correct after deletion, retraction and restart. Distinguish derivation evidence from external source truth. |
| SEM-4 / PLANNED | Shape/inference policy and relation cardinality | QRY-3, TXN-1 | Define asserted/inferred and write-time/explicit validation modes; test lower/upper relation counts, inference and retraction with actionable violations. Do not imply full SHACL conformance. |
| SEM-7 / PLANNED | `sameAs` preview, conflicts and merge record | OPS-1, QRY-3, AGT-4 | Preview IDs/relations/constraints, test conflict policy and deterministic canonical IDs, retain merge records and apply under revision/authorization checks. Undo needs a separate preservation/split design and remains unsupported. |
| SEM-8 / PLANNED | Assertion origin and trust policy | Design now with API-2; full acceptance needs OPS-2, QRY-3, TXN-2 | Distinguish asserted/derived state from source lineage, ingestion actor and model origin. Missing source remains unknown; explicit policy determines eligibility as a rule premise. Apply the same policy to text, structured and bulk writes. Define migration for existing assertions; preserve lineage through updates/deletion, restart and backup/restore. Recording origin does not certify truth. |

Functional/inverse-functional properties (SEM-1) and general chains (SEM-2) are
GATED on a use case, bounded execution, provenance and retraction design.
Distributed materialization (SEM-5) also requires supported distribution.

### Stage 4 — SDKs, import, and protocol adapters

| ID / status | Work | Dependencies | Acceptance |
|---|---|---|---|
| API-3 / PLANNED | One official Python or TypeScript SDK | API-1–2, AGT-1–2; expose AGT-3/4 only after their acceptance | Select a language from agent-integrator needs; test version pairs, renewal, `429`/`Retry-After`, cancellation and typed results. Preserve outcome/permission semantics and reject unsupported operations. |
| API-4 / PLANNED | Import/query semantic parity | QRY-3, OPS-1, SEM-4 | Compare bulk and ordinary writes for validation, IDs, indexes, counters, history, inference and shapes. Any bypass needs an explicit mode plus a tested rebuild/validation step. |
| AGT-5 / PLANNED | Protocol/MCP adapter parity and certification | API-1–2, AGT-1–2, EVAL-1; mutation adapters additionally AGT-3/4 and SEC-3 | Run shared fixtures across HTTP/gRPC, SDK and selected agent adapters. Preserve types, permissions, completeness, budgets, evidence and mutation outcomes. Define adapter ownership/version support; a server may live in `byori`, and transport availability cannot imply engine support. |

### Stage 5 — operational acceptance

| ID / status | Work | Dependencies | Acceptance |
|---|---|---|---|
| SEC-5 / PLANNED | Operational endpoint and lifecycle policy | SEC-1, SEC-3, OPS-1–2 | Explicit metrics/diagnostics access and retention, access tests, dashboards/alerts backed by runtime metrics, and a tested recovery/upgrade runbook. Verify operator observability without leaking agent credentials, schema or evidence. |

### Stage 6 — correctness- and agent-evaluation-driven performance

| ID / status | Work | Dependencies | Acceptance |
|---|---|---|---|
| PERF-1 / PLANNED | Correctness-checked benchmarks | Baselines now; align acceptance with QRY-1–2 and EVAL-1 | Validate results/failures before throughput; real concurrent requests; record dataset, seed, hardware, build, warmup, latency distribution, memory and durability mode. Report agent task success separately from engine speed. |
| PERF-2 / PLANNED | LDBC batch-read acceptance ([#10](https://github.com/byoridb/byoridb/issues/10)) | PERF-1 | Convert external Q9 to existing batching; verify result equality and measure the issue's `<10s` target at its specified scale/environment. Engine batching already exists. |
| PERF-3 / GATED | Index, planner, parallelism or streaming optimization | PERF-1–2, EVAL-1; measured bottleneck | Establish baseline/control results and compare correctness, task success, calls/tokens, latency and memory after a scoped change. Agent cost claims require comparable successful tasks; lower cost cannot compensate for incorrect state or evidence. |

## Later extensions and entry gates

| ID / status | Direction | Entry and completion evidence |
|---|---|---|
| TMP-2 / GATED | Asserted `MATCH/GO AS OF` | OPS-1–2, QRY-2; define snapshot selection and test historical adjacency/properties/deletion/reverse edges plus current-view parity. |
| TMP-1/3/4 / GATED | Valid-time writes, history inspection, retention/GC | Establish time-axis semantics and retention needs; preserve backup/recovery and callers' history guarantees. |
| TMP-5 / GATED | Historical inference | Version rules and identity mappings as well as facts; demonstrate reproducible derivations first. |
| RDF-1 / GATED | RDF/Turtle bridge | API-4; map IRIs, blank nodes, datatypes and multi-values, then prove round-trip preservation for a documented subset. No SPARQL or full RDF-store redesign promise. |
| DIST-1–4 / GATED | Supported multi-node deployment | Demonstrated capacity/availability need and operating owner; bootstrap/Raft wiring, coherent deployment, identity revocation and multi-process failure/upgrade E2E. |
| EXEC-1 / GATED | Volcano migration | Measured intermediate-memory or LIMIT-latency pressure; use the [design](VOLCANO_MIGRATION_PLAN.md) incrementally with result parity. |

## Issue and milestone reconciliation

GitHub open issues were checked on 2026-10-30: **#10, #23, #24**. Closed issues
and merged implementation are not upcoming work; the following rows retain their
acceptance boundaries rather than duplicating the historical issue diary.

| Work | Baseline evidence / remaining owner |
|---|---|
| Unknown MATCH functions, edge UPDATE, FETCH rank, trailing input | Landed: PRs #106, #107, #109, #111; preserve regressions |
| String VID migration docs, edge index deletion, edge LOOKUP/type alternation | Landed: PRs #112–115; bounded edge ranges subsequently landed in #129 |
| GO edge/rank rows, atomic edge UPDATE indexes, explicit edge LOOKUP, edge ranges, node label AND/OR | Landed: PRs #126–130; related issues are closed |
| [#10](https://github.com/byoridb/byoridb/issues/10) | Engine batching landed; external LDBC acceptance remains PERF-2 |
| [#23](https://github.com/byoridb/byoridb/issues/23) | Engine session-status contract landed; client retry work belongs to `byori` ([byori#54](https://github.com/byoridb/byori/issues/54)) |
| [#24](https://github.com/byoridb/byoridb/issues/24) | Linux/macOS x86_64/arm64 archives exist; settle Windows support policy, graceful shutdown and release tests before advertising support |
| Historical COR-1/2/3 | Standalone string VIDs, identity listings and license/archive packaging landed; Windows policy remains separate |

## Verification and maintenance

### Foundations implementation — 2026-10-30

The foundations commits through `20d464b`, integrated into `main`, add four
independently reviewable slices:

- **DEP-1:** compatible dependency patches and mandatory audit in automatic
  version-bump/release paths. Compression remains enabled; actual gzip/zstd/plain
  gRPC requests and responses retain exact UTF-8 values.
- **QRY-2 partial:** MATCH full/tag-VID/property-index candidate scans probe cap+1;
  over-budget scans and variable-length traversal fail explicitly. Index partition
  and group-count errors propagate. Malformed tag-index encodings fail rather than
  hide the cap sentinel or become phantom IDs. Zero budget remains unbounded;
  query LIMIT/OFFSET semantics are preserved, without overriding scan budgets.
- **AGT-1/API-2 partial:** read-only discovery contract 1 with a shared authenticated
  service path, bounded schema, fixed errors and HTTP/gRPC parity. Effective roles
  are global; snapshot/revision and proposed write contracts remain unsupported.
- **EVAL-1 partial:** two versioned INT64/string-ID task fixtures check discovery,
  exact answers, access denials and unchanged final person rows in fresh in-memory
  databases. No model evaluation or performance advantage is claimed.

Compression validation also exposed a long-literal lexer stack overflow with a
213KB string before any network/compression processing. An iterative quoted-string
scanner fixes that path while retaining escape, Unicode and location semantics.
New tests process large literals on a 128KiB stack; transport tests now use 213KB
individual strings and four result rows without increasing the test stack.
This does not establish a bound for every parser nesting/depth shape.

MATCH candidate/traversal regressions first produced 8 failures and 5 passing
controls; the malformed index sentinel case independently reproduced a successful
partial result before its fix. Final focused checks pass: MATCH 55, storage 59,
discovery 15, parser 136, compression 3, and EVAL fixtures 2. Whole-workspace results
are recorded below. The release workflow's
failure simulation confirms an audit failure prevents the initial push and any
post-rebase push; hosted GitHub Actions and release publication were not run here.


| Final local gate | Result |
|---|---|
| `cargo fmt --all -- --check` | PASS |
| `cargo check --locked --workspace --all-targets --all-features` | PASS |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo test --locked --workspace --all-features -- --test-threads=1` | PASS: 1,057 passed, 8 ignored across 36 suites; ignored cases were not run |
| `cargo build --locked --workspace --release` | PASS |
| `cargo audit --json` (local cargo-audit 0.22.1) | PASS: zero vulnerabilities, zero unsound/yanked warnings; bincode unmaintained warning retained |
| `mdbook build book` and `mdbook build book/ko` | PASS |
| Documentation parity and local links | PASS: 10 rendered pages, 204 links/anchors, 9 EN/KO code-example pairs and 34 task/status rows |
| `git diff --check` | PASS |

These are local acceptance results, not a hosted CI or release result. The
workflow keeps its pinned cargo-audit 0.22.2; the local audit used 0.22.1.

### Agent-first design revision — 2026-10-30

The product design, public entry points, architecture summary, contribution
guidelines, and delivery priorities were revised together in both languages.
Both mdBook builds, local file/rendered-link checks, EN/KO task and example
parity, and `git diff --check` pass. This revision changes documentation and
book metadata only; it does not implement the proposed agent contracts.
No Rust gates or agent/model evaluations were rerun for this documentation
revision. The earlier QRY-1 implementation evidence remains below; DEP-1 was
still unresolved at that checkpoint and is updated in the foundations record.

### QRY-1 branch verification — 2026-10-30

The original seven focused regressions produced five failures and two passing
controls before the fix. After the fix and an additional chained-OPTIONAL
regression, all **eight** pass. The checks for that prior implementation slice were:

| Check | Result |
|---|---|
| `cargo fmt --all -- --check` | PASS |
| `cargo check --locked --workspace --all-targets --all-features` | PASS |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo test --locked --workspace --all-features -- --test-threads=1` | PASS: 1,018 passed, 8 ignored; ignored cases were not executed |
| `cargo build --locked --workspace --release` | PASS |
| `mdbook build book` and `mdbook build book/ko` | PASS |
| Changed rendered documentation links and EN/KO book paths | PASS: 410 local links/anchors checked; 23 source page paths match |
| `cargo audit` | FAIL: 10 baseline vulnerabilities; tracked in DEP-1, with no dependency or lockfile changes in QRY-1 |

This records local validation of the prior QRY-1 slice, not a passing CI run,
merge, release, or validation of the newly planned agent contracts.

### Per-task delivery checks

For each task, add a failing behavioral/fault regression where practical,
implement, run focused checks, update both languages, then record actual commands
and outcomes before DONE. Branch work is not released until it lands on the
release line. Preserve baseline and branch evidence separately.

Keep H-1–H-6 regressions, current/history consistency, string IDs, indexes and
inference retraction covered. Authentication changes need nested authorization,
durable hydration, revocation, redaction and concurrency checks. Raft changes
also require distributed E2E; library tests alone do not prove operational support.

```bash
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features -- --test-threads=1
cargo build --locked --workspace --release
cargo audit
mdbook build book
mdbook build book/ko
```

Run checks appropriate to the change and required repository gates; explicitly
record unavailable/unrun checks. Check EN/KO book paths and local links after doc
reorganization. Do not promote archived test counts, deployment health, audit
results or benchmark numbers into current evidence.

# Roadmap

**English** | [한국어](../ko/development/roadmap.html)

Updated 2026-10-31 against released **v0.4.22** (`main` at `6851936`). ByoriDB is developing into an
**agent-first, general-purpose graph and ontology database**. Agents are direct
consumers; people operate, evaluate, govern and purchase deployments. Agent memory
is one use case. The goal is correct task completion with inspectable evidence and
reliable state changes; agent cost or speed advantages still need measurement.

The [product philosophy and design](https://github.com/byoridb/byoridb/blob/main/docs/PRODUCT_DESIGN.md)
define the target contracts. The delivery source of truth is
[docs/PLAN.md](https://github.com/byoridb/byoridb/blob/main/docs/PLAN.md).
This is an approved direction and acceptance plan, not a release schedule.
Read-only discovery contract 1 and the foundations correctness fixes are released.
Broader agent contracts remain PLANNED or IN PROGRESS; an unfinished task may
contain released slices, as recorded in the plan.

[CI](https://github.com/byoridb/byoridb/actions/runs/41054788495) and
[Auto-release](https://github.com/byoridb/byoridb/actions/runs/41054789050) succeeded.
The [v0.4.22 release](https://github.com/byoridb/byoridb/releases/tag/v0.4.22) has four
Linux/macOS x86_64/arm64 `.tar.gz` artifacts. The previous v0.4.21/pre-foundations
snapshot is historical; active QRY-2 branch work below is not yet released.

## Available in v0.4.22

- Vertex and edge CRUD, `FETCH`, `GO`, `MATCH`, tag/edge `LOOKUP`, path finding,
  `EXPLAIN` and `PROFILE`. Edge updates and eligible local edge-index ranges are
  implemented; node labels support conjunction and disjunction.
- INT64 and standalone string VIDs for graph CRUD/traversal, tag/edge/reverse
  indexes, and persisted HNSW recommendation indexes.
- Class hierarchies, disjoint/equivalent classes, semantic edge rules, two-link
  property chains, current-view inference, edge `WHY`, DRed retraction and shapes.
- Structural, embedding and blended recommendations.
- Asserted current/history atomic writes and vertex/edge `FETCH ... AS OF`, plus
  full snapshot backup and restore that preserve both tables.
- A single-node redb server with shared HTTP/gRPC authentication, durable users,
  built-in roles, local revocation, configurable login throttling and session TTL.
- Authenticated read-only discovery contract 1 over HTTP/gRPC: bounded schema,
  global-role permissions, explicit capabilities and fixed discovery errors.
- `OPTIONAL MATCH` error propagation and explicit MATCH candidate/variable-length
  cap errors; index-partition failures propagate and malformed tag-index keys fail.

These capabilities have limits. `WHY` and recommendation still need string VID
parity. `sameAs` merging is irreversible. Compound statements are not transactions.
At that release baseline, scan and inference limits do not have a uniform
complete-or-error contract.
Backup metadata verification is not a full content check or restore drill.
Native TLS, space-scoped grants and a supported multi-node runtime are absent.
Selected nGQL/OWL-style features do not imply full language/standard conformance.

## Next delivery stages

| Order | Outcome | Acceptance focus |
|---|---|---|
| Release requirement — completed | DEP-1 DONE, released in v0.4.22 | Zero vulnerabilities at acceptance; one bincode unmaintained warning remains. Automatic version bump/release paths require audit; future releases must pass again |
| 1 | Correctness and machine contracts (QRY-1–4, API-1–2, AGT-1–2, EVAL-1) | Complete results or explicit errors, inference recovery, actual cancellation; typed parameters/errors, authorized discovery, bounded results/evidence and an early agent evaluation baseline |
| 2 | Recovery, reliable writes and access boundaries (OPS-1–2, TXN-1–2, SEC-3/1, AGT-3–4) | Restore drills and versioned storage; explicit compound outcomes then atomic DML; early space grants/TLS; durable idempotent outcomes, revisions and preview/apply |
| 3 | Ontology consistency and evidence (SEM-6/3/4/7/8) | String VID parity, vertex-type explanations, shape/inference and cardinality policy, revision-checked `sameAs` preview/merge records, assertion origin and explicit premise-trust policy |
| 4 | SDKs, import and adapters (API-3–4, AGT-5) | One official SDK, bulk-import semantic parity, shared protocol/MCP adapter acceptance fixtures and explicit ownership |
| 5 | Operational acceptance (SEC-5) | Metrics/diagnostics access and retention, observable runtime behavior, tested recovery/upgrade runbooks |
| 6 | Correctness- and agent-evaluation-driven performance (PERF-1–3) | Correct results and successful agent work before throughput/cost, real concurrency, LDBC batch-read acceptance, measured optimization |

**QRY-1 is DONE and released in v0.4.22.** It propagates errors returned by
`OPTIONAL MATCH` execution and pattern resolution while preserving genuine
no-match null bindings; eight focused regressions passed. The release proof is
linked above. The same release includes MATCH candidate-scan and variable-length
cap errors, index-partition error propagation, strict tag-index decoding and
iterative long-string lexing. A small query `LIMIT` cannot bypass candidate
budgets. Broader QRY-2 acceptance remains **IN PROGRESS**.

The current source consolidates LOOKUP/FIND/MATCH completeness, distributed
index continuation and remote fetch checks, inference recovery, single-statement
DML staging, backup integrity/restore drills and server lifecycle fixes. See the
[standalone acceptance record](https://github.com/byoridb/byoridb/blob/main/docs/DEPLOYMENT_READINESS.md)
for exact guarantees and executable evidence. QRY-2 remains IN PROGRESS. QRY-3,
OPS-1 and SEC-1 are DONE at source acceptance, with release publication pending.
Local gates and real recovery/TLS drills passed; hosted workflows and archives
for the final commit must still confirm publication. The
[TLS deployment profile](../operations/secure-deployment.md) is the supported
network boundary. A single-statement transaction does not complete TXN-2's future
multi-statement/inference contract. Typed operations, idempotency and revisions
remain planned; the release workflow must pass before publication is claimed.

## Machine-contract boundaries

Current HTTP/gRPC query execution supports `read_only`; edge `WHY` returns string
columns. Typed query parameters, structured proof envelopes, durable
idempotency, revision checks and preview/apply are planned. A separate MCP server
may belong to `byori`; adapter availability does not create engine capabilities.
Adapters must preserve authorization, types, completeness, budgets and outcomes.

A proposed partial-result mode is not implemented: it would require explicit
opt-in and machine-readable reasons. Page completion, query completeness and
inference status must be distinct; snapshot support must be declared accurately.
A write error is not proof of rollback. Idempotency requires an atomic commit of
the mutation, scoped request key and durable outcome; unknown/expired outcomes
cannot authorize blind retry. Preview never grants authority: apply rechecks
permissions and revisions and rejects stale or expired plans without side effects.
An explanation proves derivation from recorded facts/rules, not source truth.

## Later extensions need explicit entry criteria

- **Historical graph queries:** start with asserted `MATCH/GO AS OF` after
  recovery and snapshot semantics are established. Historical inference also
  needs versioned rules and identity mappings. Valid-time writes and retention
  require separate contracts.
- **RDF/Turtle interchange:** define a supported mapping for IRIs, blank nodes,
  datatypes and multi-values and prove round-trip preservation. This is not a
  commitment to SPARQL or a full RDF-store redesign.
- **Distribution:** require an actual capacity/availability need and an operating
  owner. Bootstrap, Raft/storage wiring, coherent deployment, authorization and
  multi-process failure/upgrade tests must pass before advertising support.
- **Volcano/parallel execution:** start from measured intermediate-memory,
  LIMIT-latency or throughput bottlenecks, with result parity throughout.

## Existing issue boundaries

[#10](https://github.com/byoridb/byoridb/issues/10) has engine batching implemented;
external LDBC harness acceptance remains. The engine part of
[#23](https://github.com/byoridb/byoridb/issues/23) is implemented; client retry work
belongs to `byori`. [#24](https://github.com/byoridb/byoridb/issues/24) concerns the
remaining Windows support policy and implementation. Linux/macOS x86_64/arm64
release artifacts already exist.

Choose a scoped task from the detailed plan and follow the
[contribution guide](contributing.md). Include behavioral acceptance tests and
update English/Korean documentation together. Historical design and incident
notes are archived in `docs/archive/`; they do not describe a live deployment.

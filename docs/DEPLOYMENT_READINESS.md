# Standalone release acceptance

> **English** | [한국어](DEPLOYMENT_READINESS.ko.md)
>
> Verified release **v0.4.23**, published 2026-10-31 at 07:06:25 UTC.
> Local checks, hosted gates and published-artifact evidence are distinguished below.

The first supported deployment is one standalone server and redb volume, with
HTTP and gRPC sharing authentication, sessions and execution. Agents consume
authorized discovery and text queries. [PLAN.md](PLAN.md) retains the remaining
typed-operation, idempotency, revision, SDK and multi-node roadmap.

## Release acceptance — v0.4.23

QRY-3, OPS-1 and SEC-1 are **DONE (released v0.4.23)** for the standalone
contracts below. Implementation commit
[`2cb4072d0314bc911e9165c218d87feeacadbc10`](https://github.com/byoridb/byoridb/commit/2cb4072d0314bc911e9165c218d87feeacadbc10)
passed [CI](https://github.com/byoridb/byoridb/actions/runs/41177637017).
The version-bump/tag commit
[`fb96cb3751934f447833b7ce97e3230459be1830`](https://github.com/byoridb/byoridb/commit/fb96cb3751934f447833b7ce97e3230459be1830)
passed [Auto-release](https://github.com/byoridb/byoridb/actions/runs/41177636969)
and is the target of the [published v0.4.23 tag](https://github.com/byoridb/byoridb/releases/tag/v0.4.23).
QRY-2 remains IN PROGRESS for the broader completeness/result-envelope work.

| Check | Observed result |
|---|---|
| Formatting, locked workspace check and Clippy | PASS with all targets/features; Clippy uses `-D warnings` |
| Serial locked workspace tests | Local, hosted CI and exact-ref release gate each report **1,272 passed, 0 failed, 8 existing ignored across 37 suites** |
| Dependency audit | **0 vulnerabilities**; the existing `bincode` 1.3.3 unmaintained warning remains the tracked format-migration boundary |
| Docker TLS deployment | Source and prebuilt image drills PASS; final integrated source `--build` drill PASS. Real HTTPS/gRPC authentication, private backend, denied operations routes, certificate rotation, restart persistence and log redaction checked |
| Independent release/profile review | No blocking findings after fixes |
| Native release build | PASS in **1 min 43 s** |
| Real native binary smoke | PASS: authenticated HTTP/gRPC queries, durable restart, backup/verify/restore and restored-data reads |
| Recovery fixture timings | `data.redb`: **294,912 bytes**; create **0.084 s**, verify **0.028 s**, restore **0.055 s** |
| Startup fixture timings | Initial **0.267 s**, restart **0.237 s**, restored data **0.244 s** |
| Documentation | English then Korean mdBook builds and 14-pair code/status/link checks PASS |
| Hosted publication | **PASS**: CI and auto-release completed successfully; four Linux/macOS x86_64/arm64 archives, each with server/CLI/backup plus LICENSE/NOTICES. macOS binaries signed/notarized; extracted runtime smoke passed for every target, natively on Linux and through Rosetta for macOS x86_64 |
| Published-asset verification | **PASS**: downloaded all four archives, matched release-asset SHA-256 values, checked exactly five members and all 12 binary architectures; verified six macOS signatures. Downloaded macOS arm64 binaries report v0.4.23/`fb96cb3` and pass HTTP/gRPC restart and backup/restore smoke |

Local build/recovery timings above are measurements of the acceptance fixture;
they are not production throughput or universal RPO/RTO. The hosted workflow
links identify the release tested and published, rather than a later docs-only commit.

## Implemented acceptance boundaries

| Area | Contract | Executable evidence |
|---|---|---|
| Query completeness | LOOKUP/FIND/MATCH budget overflow and storage/decode errors fail explicitly; SQL LIMIT does not disguise an incomplete candidate scan | Executor completeness suites, including real Meta + two Storage RPC fixtures |
| Distributed components | Validate all partition identities/hosts, page each index with a query-bound keyset cursor, fetch remote payloads and reject failed partitions/corrupt records | `distributed_completeness_tests.rs`; these tests do not make a supported replicated launcher |
| DML | Stage one statement's current data, asserted history, indexes, vectors, counters and sameAs changes; commit once on Memory/redb | `dml_atomic_tests.rs`: byte-exact failure/cancellation checks and concurrent DDL |
| Inference | Commit a durable incomplete marker with assertions; serialize reads/writes; rebuild closure before clearing it | `inference_recovery_tests.rs`: faults, cancellation, concurrent repair and redb reopen |
| Recovery | Verify manifest, digest, size and redb contents on an isolated copy; verify staged restore before replacing a stopped target | Backup failure injection and `tests/backup_recovery_test.rs` actual CLI drill |
| Lifecycle | Bind both APIs before storage/readiness; supervise unexpected exit; count requests before auth waits; propagate checkpoint/drain failure | Graph admission races, `server_cli_test.rs`, checkpoint fault test, actual binary smoke |
| Transport | TLS HTTP/gRPC proxy, unpublished backend ports, public operations routes blocked, non-root containers, certificate rotation | `deploy/secure/smoke.py --build` |
| Packaging | Exact-ref audit/fmt/check/Clippy/tests, required server/CLI/backup, four archives with license/notices | CI/release workflows and `scripts/release_smoke.py` on unpacked artifacts |

The archive smoke authenticates over both protocols, writes and reads an indexed
fact, rejects a read-only mutation and unsupported HTTP parameters, stops with
SIGTERM, restarts, creates/verifies/restores a backup and reads the same fact.
It also checks that credentials and query values are absent from server logs.
Linux targets run natively; macOS x86_64 runs through Rosetta on ARM runners.

## Inference operation and uncertain outcomes

```ngql
USE my_space;
CHECK INFERENCE;
REPAIR INFERENCE;
```

`CHECK INFERENCE` requires ordinary read permission and returns `complete` or
`repair_required`. `REPAIR INFERENCE` requires GOD/ADMIN and is refused by a
read-only request, including compound and PROFILE wrappers. A dirty space rejects
graph reads and further mutations until repair succeeds. Repair is repeatable,
preserves asserted history, removes stale derived/reverse/type/provenance entries
and reconstructs the current closure. It is an O(graph) operation; automatic
semantic mutation completion uses the same full-rebuild strategy.

Assertion commit and closure completion are separate transactions. An inference
error, cancellation or lost response does **not** prove the assertion rolled back.
Inspect status and current facts after repair before deciding whether to retry.
There is no durable idempotency key or mutation-outcome API. Compound statements
commit clause by clause; the single-statement DML guarantee is not a compound
transaction or an all-effects inference transaction. Embedded custom KV backends
must implement atomic `batch_apply` to provide the Memory/redb guarantee.

Updating an inferred-only edge explicitly promotes it to an assertion; deleting
one directly is rejected. Delete supporting assertions instead. Users cannot set
the reserved `__inferred__` marker. SameAs merges remain irreversible. Referenced
classes cannot be dropped (subclass, equivalent/disjoint, DOMAIN/RANGE, shape or
tag index); with no ALTER CLASS, mutually equivalent classes cannot be removed
individually while those references exist.

## Deployment and recovery limits

- Use the [TLS deployment runbook](../book/src/operations/secure-deployment.md).
  Global roles do not isolate mutually untrusted tenants; use separate stores.
  The launcher rejects non-default cluster settings. Native TLS and HA remain
  unsupported, and the legacy AKS manifests do not supply an encrypted ingress.
- Float equality preserves existing epsilon comparison by rechecking the entire
  selected index; its scan budget counts all candidates. Int/Float equality
  remains type-strict. Index continuation is not a snapshot across concurrent
  writes; distributed full scans/range queries retain their existing guards.
- Offline backup verification detects corruption, not external-source truth or
  archive authenticity. See [backup and restore](../book/src/operations/backup.md).
  Two-directory promotion is rollback-safe for ordinary errors but not crash-atomic;
  retain `.byoridb-restore-*/previous` after an interrupted overwrite.
- HTTP query bodies reject unknown fields with 422. General typed parameters,
  exact JSON INT64 representation for all values, and general result/evidence
  envelopes remain separate API work. Session IDs already use decimal strings.
- Bound result/staging/scan limits for the workload. No throughput or recovery-time
  promise is inferred from correctness fixtures; benchmark representative data.

## Verification commands

```bash
cargo fmt --all --check
cargo check --locked --workspace --all-targets --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features -- --test-threads=1
cargo build --locked --release --workspace
cargo audit --file Cargo.lock
BYORIDB_RELEASE_VERSION="$(python3 -c 'import pathlib,tomllib; print(tomllib.loads(pathlib.Path("Cargo.toml").read_text())["workspace"]["package"]["version"])')"
python3 scripts/release_smoke.py --bin-dir target/release --version "$BYORIDB_RELEASE_VERSION"
python3 deploy/secure/smoke.py --build
mdbook build book
mdbook build book/ko
```

The version command requires Python 3.11 or newer. Published builds
also pass `--commit` to the archive smoke to require a clean exact build SHA.
Check the [CI runs](https://github.com/byoridb/byoridb/actions/workflows/ci.yml),
[auto-release runs](https://github.com/byoridb/byoridb/actions/workflows/autorelease.yml)
and [published artifacts](https://github.com/byoridb/byoridb/releases) for that SHA.

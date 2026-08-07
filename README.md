# ByoriDB

[English (default)](README.md) | [한국어](README.ko.md)

<p align="center">
  <img src="book/src/assets/byoridb-icon.png" alt="ByoriDB official icon" width="256">
</p>

> A semantic graph database written in Rust, with ontology inference,
> provenance, and point-in-time history.

ByoriDB combines a property-graph core with a semantic layer. It provides an
nGQL-style query surface, write-time ontology materialization, explanations for
inferred edges, and temporal reads from one standalone server process.

> [!NOTE]
> Looking for the local knowledge graph and agent-memory product for Claude
> Code, Codex, and other coding agents? See
> **[byoridb/byori](https://github.com/byoridb/byori)**. This repository is the
> general-purpose database engine used underneath it.

> [!CAUTION]
> The standalone, single-node launcher is the primary supported path. The
> repository contains distributed components, but storage/Raft peer bootstrap
> and multi-node operational wiring are not complete. Do not treat the current
> cluster mode as production-ready.

## Capabilities

- **Property graph and nGQL-style queries:** `MATCH`, `GO`, `FETCH`, `LOOKUP`,
  `FIND PATH`, schema operations, and data mutation.
- **Ontology inference:** write-time materialization for class hierarchies and
  selected RDFS-Plus/OWL 2 RL-style rules, including transitive, symmetric,
  inverse, subproperty, equivalent-property, and two-link property-chain rules.
- **Provenance:** `WHY` explains the rule and premises behind an inferred edge;
  provenance supports incremental retraction when asserted edges are deleted.
- **Identity merging:** explicit `sameAs` edges perform an irreversible
  canonical merge. This is intentionally narrower than complete OWL semantics.
- **Temporal history:** asserted vertex and edge writes update the current view
  and append history atomically. `FETCH PROP ... AS OF <epoch-ms>` reads a
  vertex or edge at a point in time.
- **Similarity:** structural Jaccard, embedding, and hybrid recommendation.
- **Operations:** HTTP and gRPC APIs, an interactive CLI, backup/restore,
  readiness checks, and Prometheus metrics.
- **Pure-Rust storage:** redb provides the embedded KV layer; no C++ toolchain is
  required.

### Batch-read behavior and limits

`FETCH PROP ON <tag> vid, ...` sends all current-view vertex keys through one
storage `batch_get`. Results retain the input VID order; missing VIDs are
omitted. The HTTP query-text limit is 1 MiB, so a typical 500–1,000 numeric-VID
request is comfortably below the transport limit. Response size still depends
on projected property payloads and remains subject to the configured query
result-memory cap.

For `GO ... YIELD $$.tag.prop` and `YIELD vertex`, ByoriDB deduplicates the
destination VIDs and loads them in one batch before projection. Missing tags,
properties, or destination vertices project as `NULL`. `EXPLAIN` and `PROFILE`
show this work as `GetVertices` with `batch destination projection` detail.

ByoriDB does not implement the complete OWL 2 RL rule set or a general temporal
query language. In the current temporal model, valid time and transaction time
are generated together by the server; temporal `MATCH`, `GO`, `BETWEEN`, and
user-supplied `VALID FROM/TO` are not supported. Inference uses the current view
and does not reconstruct historical inferred facts. See
[the implementation plan](docs/PLAN.md) for detailed status and constraints.

## Architecture

The codebase is organized around three service components:

- **Graph service** (`byoridb-graph`) parses, authorizes, plans, and coordinates
  queries. Authentication and session state are process-local while the server
  is running.
- **Meta service** (`byoridb-meta`) contains space, schema, partition, and
  related metadata services used by the partial cluster path.
- **Storage service** (`byoridb-storage`) stores vertices and edges and contains
  the partitioning and custom Raft implementation.

The supported standalone launcher starts embedded storage and the Graph HTTP
and gRPC listeners in one process. It starts the Meta gRPC server only when
`cluster.peers` is configured; doing so does not make the incomplete
multi-node path production-ready. User records are durable, but live sessions
are not shared across processes. Restarting a server invalidates its sessions,
and current multi-instance deployments do not coordinate session revocation.

## Quick start

### Release archives

The latest published release is
[v0.3.3](https://github.com/byoridb/byoridb/releases/tag/v0.3.3), with archives
for Linux x86_64 and macOS on Intel and Apple Silicon. That tag predates the
current `main` branch's authentication hardening, shared HTTP/gRPC session
state, temporal v1.1 changes, and edge `AS OF` reads. Build from a pinned
`main` commit if you need the behavior documented below, and identify deployed
artifacts by tag or commit SHA.

See [Installation](book/src/getting-started/installation.md) for the current
ARM Linux, Windows, macOS signing, and archive-license limitations.

### Prerequisites

- Linux or macOS
- Rust 1.90, pinned by `rust-toolchain.toml`
- `protobuf-compiler` for gRPC code generation

### Build and run

The current standalone server refuses to start unless
`BYORIDB_ROOT_PASSWORD` is set to a non-empty value. Use a secret manager for
deployments and a strong local secret for development.

```bash
git clone https://github.com/byoridb/byoridb.git
cd byoridb
cargo build --locked --workspace --release

export BYORIDB_ROOT_PASSWORD='replace-with-a-strong-secret'
cargo run --locked --release --bin byoridb-server
```

The default listeners are gRPC on `0.0.0.0:9669` and HTTP on
`0.0.0.0:19669`.

```bash
curl --fail http://127.0.0.1:19669/health
curl --fail http://127.0.0.1:19669/ready
```

In another terminal, start the CLI:

```bash
export BYORIDB_USER=root
export BYORIDB_PASSWORD='replace-with-a-strong-secret'
cargo run --locked -p byoridb-client --bin byoridb-cli
```

The CLI has no default credentials; both the user and password must be supplied
through these environment variables or command-line flags.

For SQL examples and the HTTP session flow, continue with the
[quick-start guide](QUICKSTART.md).

## Development checks

Integration tests must run serially because temporary redb databases can
otherwise contend for files and locks.

```bash
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features -- --test-threads=1
cargo build --locked --workspace --release
```

CI also audits the committed `Cargo.lock` against RustSec advisories.

See [CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull request.

## Security and deployment boundaries

ByoriDB currently provides no native TLS termination and no general-purpose
login rate limiter. Put HTTP and gRPC behind trusted TLS termination, restrict
network access, and add rate limiting before exposing either listener.

The built-in roles apply their permissions to `*` (every space), and the
current query language has no space-scoped `GRANT` syntax. They are therefore
not a tenant-isolation boundary. Treat session IDs as bearer credentials and do
not place them in logs.

See [SECURITY.md](SECURITY.md) for the supported security model, deployment
checklist, and private vulnerability-reporting instructions.

## Documentation

- [Quick start](QUICKSTART.md)
- [User and operations guide](book/src/SUMMARY.md)
- [Implementation status, constraints, and roadmap](docs/PLAN.md)
- [Contributing](CONTRIBUTING.md)
- [Security policy](SECURITY.md)
- [Open-source notices](NOTICES.md)
- [Korean documentation index](README.ko.md)

## License

ByoriDB is licensed under the [Apache License 2.0](LICENSE).

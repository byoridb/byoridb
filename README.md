# ByoriDB

[한국어](README.ko.md)

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

ByoriDB does not implement the complete OWL 2 RL rule set or a general temporal
query language. In the current temporal model, valid time and transaction time
are generated together by the server; temporal `MATCH`, `GO`, `BETWEEN`, and
user-supplied `VALID FROM/TO` are not supported. Inference uses the current view
and does not reconstruct historical inferred facts. See
[the implementation plan](docs/PLAN.md) for detailed status and constraints.

## Architecture

The standalone binary hosts three logical services in one process:

- **Graph service** (`byoridb-graph`) parses, authorizes, plans, and coordinates
  queries. Authentication and session state are process-local while the server
  is running.
- **Meta service** (`byoridb-meta`) manages spaces, schemas, partitions, and
  related metadata.
- **Storage service** (`byoridb-storage`) stores vertices and edges and contains
  the partitioning and custom Raft implementation.

User records are durable, but live sessions are not shared across processes.
Restarting a server invalidates its sessions, and current multi-instance
deployments do not coordinate session revocation.

## Quick start

### Prerequisites

- Linux or macOS
- Rust 1.90, pinned by `rust-toolchain.toml`
- `protobuf-compiler` for gRPC code generation

### Build and run

The standalone server refuses to start unless `BYORIDB_ROOT_PASSWORD` contains
a nonblank value. Use a secret manager for deployments and a strong local
secret for development.

```bash
git clone https://github.com/byoridb/byoridb.git
cd byoridb
cargo build --release

export BYORIDB_ROOT_PASSWORD='replace-with-a-strong-secret'
cargo run --release --bin byoridb-server
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
cargo run -p byoridb-client --bin byoridb-cli
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
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features -- --test-threads=1
```

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

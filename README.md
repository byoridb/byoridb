# ByoriDB

[English (default)](README.md) | [한국어](README.ko.md)

<p align="center">
  <img src="book/src/assets/byoridb-icon.png" alt="ByoriDB official icon" width="256">
</p>

> A graph database for agent memory: facts an agent can query, explain, and
> change with explicit outcomes.

AI agents lose context between sessions. Retrieval-only memory returns similar
text, and a plain graph store keeps relationships but cannot tell an agent why a
fact holds or what it looked like earlier. ByoriDB targets that gap. An agent
stores structured facts and relations, derives new facts from a small set of
explicit rules, asks `WHY` an inferred fact holds, and reads past state with
`AS OF`. Explicit outcomes and safe retries for every change are planned, not
yet implemented.

AI agents are the primary callers. Humans configure access, inspect behavior,
and operate the service. The engine remains a general property graph, so other
workloads such as dependency analysis can use it, but agent memory sets the
priorities.

ByoriDB is not an OWL or RDF store. Its inference is a bounded set of
deterministic rules that borrow RDFS/OWL vocabulary. Complete OWL semantics,
class expressions, and an IRI/RDF data model are non-goals.

The current Rust implementation combines a property graph with a rule layer:
nGQL-style queries, write-time rule materialization, inferred-edge
explanations, and asserted-fact temporal reads in one standalone server process.
The [product philosophy and design](docs/PRODUCT_DESIGN.md) defines the target
agent contract; [the development plan](docs/PLAN.md) distinguishes implemented
query/discovery behavior from planned structured operations and reliable retries.

> [!NOTE]
> Looking for the local knowledge graph and agent-memory product for Claude
> Code, Codex, and other coding agents? See
> **[byoridb/byori](https://github.com/byoridb/byori)**. Byori is an agent-memory
> application and MCP integration built on this engine. Its
> domain-specific tools are distinct from the engine's planned agent contract.

> [!CAUTION]
> The standalone, single-node launcher is the primary supported path. The
> repository contains distributed components, but storage/Raft peer bootstrap
> and multi-node operational wiring are not complete. Do not treat the current
> cluster mode as production-ready.

## Capabilities

- **Property graph and nGQL-style queries:** `MATCH`, `GO`, `FETCH`, `LOOKUP`,
  `FIND PATH`, schema operations, and vertex/edge mutation, including edge
  `UPDATE` and indexed edge `LOOKUP`.
- **Vertex IDs:** standalone spaces support `INT64` and `FIXED_STRING(n)` IDs;
  `RECOMMEND` remains `INT64`-only and `WHY` accepts integer IDs.
- **Rule-based inference:** write-time materialization for class hierarchies
  (including equivalent classes and a `DISJOINT WITH` consistency check),
  domain/range typing, and property rules: transitive, symmetric, inverse,
  subproperty, equivalent-property, and two-link property-chain. The rules use
  RDFS/OWL vocabulary but ByoriDB is not OWL-conformant.
- **Provenance and recovery:** `WHY` explains an inferred edge’s rule and premises.
  Semantic changes rebuild current closure. `CHECK INFERENCE` reports incomplete
  state; an administrator can run `REPAIR INFERENCE` before reads resume.
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

ByoriDB does not implement OWL semantics (class expressions, cardinality,
open-world reasoning), IRIs, RDF/SPARQL compatibility, or a general temporal
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
and gRPC listeners in one process. It rejects non-default cluster settings;
the partial Meta/Raft components do not constitute a supported multi-node
deployment. User records are durable, but live sessions
are not shared across processes. Restarting a server invalidates its sessions,
and current multi-instance deployments do not coordinate session revocation.

## Quick start

### Release archives

The [latest release page](https://github.com/byoridb/byoridb/releases/latest)
provides archives for Linux x86_64/arm64 and macOS x86_64/arm64.
The macOS binaries are signed and notarized. Each archive contains the server,
query CLI, offline backup tool, license and notices. The current release workflow requires runtime checks of
the extracted binaries, including authentication, durable restart and restore.
Older archives do not gain these checks retroactively.
Use the release's successful workflow as deployment evidence, and
identify deployed artifacts by tag or commit SHA. Changes on `main` after a tag
are not included in that release.

Windows is not currently supported. See
[Installation](book/src/getting-started/installation.md) for platform support,
signature verification, and legacy archive limitations. For network deployment,
use the [private TLS profile](book/src/operations/secure-deployment.md).

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

ByoriDB provides process-local failed-login throttling, account lockout, and a
bounded Argon2 verification pool. Six throttle settings and the sliding session
TTL are configurable under `[auth]`. Native TLS termination is not implemented;
put HTTP and gRPC behind trusted TLS termination, restrict network access, and
apply connection and request limits at the edge. See
[Configuration](book/src/getting-started/configuration.md#sessions-and-login-throttling)
for the available controls.

The built-in roles apply their permissions to `*` (every space), and the
current query language has no space-scoped `GRANT` syntax. They are therefore
not a tenant-isolation boundary. Treat session IDs as bearer credentials and do
not place them in logs.

See [SECURITY.md](SECURITY.md) for the supported security model, deployment
checklist, and private vulnerability-reporting instructions.

## Documentation

- [Product philosophy and agent-first design](docs/PRODUCT_DESIGN.md)
- [Quick start](QUICKSTART.md)
- [User and operations guide](book/src/SUMMARY.md)
- [Implementation status, constraints, and roadmap](docs/PLAN.md)
- [Standalone release acceptance and recovery boundaries](docs/DEPLOYMENT_READINESS.md)
- [Contributing](CONTRIBUTING.md)
- [Security policy](SECURITY.md)
- [Open-source notices](NOTICES.md)
- [Korean documentation index](README.ko.md)

## License

ByoriDB is licensed under the [Apache License 2.0](LICENSE).

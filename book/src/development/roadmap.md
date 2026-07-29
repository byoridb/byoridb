# Roadmap

[한국어](../ko/development/roadmap.html)

This page summarizes the direction visible in the current source tree. It is
not a release schedule or a compatibility promise. `docs/PLAN.md` is the
detailed engineering source of truth; historical incident notes in that file
should not be interpreted as a statement about a live environment today.

## Available on the current main line

### Graph and query core

- spaces, tags, edge types, vertices, edges, and tag/edge indexes;
- vertex `INSERT`, `UPDATE`, and `DELETE`, plus edge `INSERT` and `DELETE`;
- `FETCH`, `GO`, `MATCH`, `LOOKUP`, and path finding;
- variable-length MATCH paths, reverse-edge traversal, grouping and common
  aggregates;
- `EXPLAIN` access-path reporting and runtime `PROFILE` observations;
- multi-row batched DML and resource guards for scans, traversal, and result
  materialization.

### Ontology and recommendations

- class hierarchies, disjointness, equivalent classes and properties;
- transitive, symmetric, inverse, subproperty, domain/range, and two-link
  property-chain semantics;
- current-view forward materialization, inference provenance, `WHY`, and
  incremental edge retraction support;
- `owl:sameAs` canonical merging with documented irreversible behavior;
- shape declarations, write-time checks, and consistency queries;
- structural, embedding, and blended recommendations, including persisted HNSW
  indexes for larger vector sets.

### Storage and temporal state

- pure-Rust redb current-view storage with protobuf vertex/edge payloads;
- a physically separate asserted-fact history table;
- atomic redb application of the current entity mutation and matching history
  version on the executor's temporal DML path;
- monotonic transaction timestamps that avoid same-millisecond history-key
  collisions;
- vertex and edge `FETCH PROP ... AS OF <epoch-ms>`, including tombstones;
- snapshot backup and restore that preserve current and history tables.

### Service and operations

- authenticated gRPC and HTTP query services sharing one service instance;
- durable non-root users, built-in roles, recursive statement authorization,
  session invalidation after security-state changes, and admin-only query
  diagnostics;
- Prometheus query metrics, health/readiness endpoints, and graceful draining;
- an interactive Rust CLI, offline CSV bulk loader, Docker assets, and a
  single-replica Azure AKS deployment definition.

## Current product boundaries

The following limitations are part of the current status, not completed
features:

- **Multi-node operation:** partition, RPC, Meta, migration, and custom Raft
  components exist, but the launcher does not wire a complete Storage/Raft
  cluster or route normal Graph queries through it.
- **Temporal semantics:** valid time is not user-specified; one epoch-ms value
  is used for valid and transaction time. Temporal `MATCH`/`GO`, intervals, and
  historical inferred facts are absent.
- **Sessions:** session and active-auth state is in process, is cleared on
  restart, and is not shared across replicas.
- **Transport security:** the server has no native TLS. Deployments need trusted
  TLS termination, network restrictions, and external traffic controls.
- **Transactions:** redb operations are transactional, but the query language
  has no general multi-statement transaction or compound rollback.
- **Edge updates:** `UPDATE EDGE` is accepted by the parser, but the current
  plan/executor path only implements vertex updates.
- **API maturity:** gRPC complex values use a JSON fallback in the structured
  response; the older JSON byte field remains for compatibility.
- **Operational packaging:** there is no supported Kubernetes operator, Helm
  chart, or automated multi-node upgrade procedure.

## Active engineering directions

### Complete the distributed runtime

The largest architecture gap is to connect and validate what is already present:

- Storage RPC and per-partition Raft startup;
- peer discovery, bootstrap, membership changes, and leader routing;
- distributed Graph execution parity across query types;
- replication/recovery/failover tests in real multi-process deployments;
- a session and authorization design for multiple Graph replicas;
- upgrade, snapshot, migration, and observability runbooks.

This work must close before documentation can recommend replicas greater than
one for shared data.

### Expand bitemporal queries

Likely next temporal increments include explicit valid-time intervals,
independent transaction-time selection, interval queries, temporal graph
traversal/pattern matching, and a defined policy for inferred history. Any
extension must preserve current-view performance and backup compatibility.

### Improve execution scalability

Parallel execution and better cost-based planning remain measurement-driven
work. Priorities include bounding memory, avoiding accidental full scans,
improving large aggregation paths, and publishing reproducible benchmarks
instead of fixed marketing QPS numbers.

### Harden operations and security

Further work includes native or formally documented TLS integration, external
rate limiting, more complete space-scoped authorization administration,
cluster-wide session revocation, backup automation, restore drills, and
metrics whose declared storage/session/partition series are fed by runtime
state.

### Mature client and wire formats

The Rust client is the implemented client surface. Rich first-class protobuf
representations for complex values and additional language clients should
follow an explicit compatibility policy rather than ad-hoc wire changes.

## How to help

Choose an issue or a concrete item in `docs/PLAN.md`, keep the change scoped,
and follow the [contribution guide](contributing.html). For distributed, Raft,
temporal, authentication, and storage changes, include the specialized
regression tests and document the remaining boundary as carefully as the new
capability.

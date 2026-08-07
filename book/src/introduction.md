# ByoriDB

[한국어](ko/introduction.html)

<p align="center">
  <img src="assets/byoridb-icon.png" alt="ByoriDB official icon" width="256">
</p>

ByoriDB is an ontology graph database written in Rust. It provides an
nGQL-inspired query language, a persistent property graph, RDFS-Plus-style
forward reasoning, recommendation primitives, and point-in-time reads for
asserted vertices and edges.

The supported deployment today is a **single ByoriDB server backed by one redb
database**. The repository also contains partition-routing, Meta/Storage RPC,
and custom Raft components, but they are not yet wired into a production-ready
multi-node launcher. See [Distributed systems](architecture/distributed.html)
before planning a clustered deployment.

## What is implemented

- **Property graph**: spaces, tags, edge types, vertices, edges, and secondary
  indexes.
- **Queries**: `FETCH`, `GO`, `MATCH`, `LOOKUP`, `FIND PATH`, `EXPLAIN`, and
  `PROFILE`.
- **Ontology features**: class hierarchies, semantic edge declarations,
  forward materialization, `owl:sameAs`, inference provenance through `WHY`,
  consistency checks, and SHACL-style shape validation.
- **Recommendations**: structural similarity, embedding similarity, persisted
  HNSW indexes for larger vector sets, filters, and blended reranking.
- **Temporal reads**: DML maintains a current view and an asserted-fact history.
  `FETCH PROP` can read a vertex or edge `AS OF` an epoch-millisecond value.
- **Authentication and authorization**: environment-backed root credentials,
  durable non-root users, built-in roles, session-based gRPC/HTTP access, and
  query authorization.
- **Operations**: Prometheus metrics, health/readiness endpoints, graceful
  shutdown, a snapshot backup CLI, Docker assets, and a single-replica AKS
  manifest.

## Runtime architecture

The standalone server composes the following crates in one process:

| Crate | Responsibility |
|---|---|
| `byoridb-common` | Shared graph values and data sets |
| `byoridb-kvstore` | redb-backed current and history keyspaces |
| `byoridb-codec` | Vertex, edge, value, and row codecs |
| `byoridb-storage` | Storage environment, indexes, RPC, partition, and Raft components |
| `byoridb-meta` | Schema, host, partition, and migration metadata components |
| `byoridb-parser` | Lexer, AST, and nGQL-inspired parser |
| `byoridb-executor` | Plans, query execution, ontology, temporal, and recommendation logic |
| `byoridb-graph` | Authentication, sessions, query service, gRPC, HTTP, and metrics |
| `byoridb-client` | Rust client and interactive CLI |
| `byoridb-bulkloader` | Offline CSV bulk loader |

The root `byoridb` package builds the `byoridb-server` and `byoridb-backup`
binaries.

## A first query

Start the server with an explicit, non-empty root secret:

```bash
export BYORIDB_ROOT_PASSWORD='replace-with-a-managed-secret'
cargo run --locked --release -p byoridb --bin byoridb-server
```

Connect from another terminal:

```bash
export BYORIDB_USER=root
export BYORIDB_PASSWORD='same-secret-used-to-start-the-server'
cargo run --locked -p byoridb-client --bin byoridb-cli
```

Then create and query a small graph:

```sql
CREATE SPACE example(vid_type = INT64);
USE example;

CREATE TAG person(name STRING, age INT64);
CREATE EDGE knows(since INT64);

INSERT VERTEX person(name, age) VALUES
  1:("Alice", 30),
  2:("Bob", 25);
INSERT EDGE knows(since) VALUES 1->2:(2024);

FETCH PROP ON person 1;
GO FROM 1 OVER knows YIELD dst(edge) AS friend;
MATCH (p:person) RETURN p LIMIT 10;
```

Continue with [Installation](getting-started/installation.html) and the
[Quick start](getting-started/quickstart.html).

## Important boundaries

- Native TLS is not implemented. Terminate TLS in a trusted proxy/load
  balancer and restrict the listener at the network boundary.
- Sessions and the active authentication cache are process-local. Sessions do
  not survive a restart and are not shared across replicas.
- `AS OF` history covers asserted vertex/edge state. It does not reconstruct
  historical inferred facts, and temporal `MATCH`/`GO` is not implemented.
- The checked-in Docker Compose services are independent standalone databases;
  they are not a three-node cluster.

## License

ByoriDB is licensed under the Apache License 2.0.

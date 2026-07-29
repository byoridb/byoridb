# Architecture overview

[한국어](../ko/architecture/overview.html)

ByoriDB is a Rust workspace whose default server composes query, metadata, and
storage logic in one process. This is the architecture that the checked-in
standalone binary and current deployment manifests actually run.

```text
                    +-----------------------+
                    | Rust CLI / gRPC / HTTP|
                    +-----------+-----------+
                                |
                    +-----------v-----------+
                    | shared GraphService   |
                    | auth, RBAC, sessions  |
                    +-----------+-----------+
                                |
                    +-----------v-----------+
                    | parser -> plan ->     |
                    | executor              |
                    +-----------+-----------+
                                |
                 +--------------v---------------+
                 | KVStore + indexes + metadata |
                 +--------------+---------------+
                                |
                    +-----------v-----------+
                    | redb/data.redb        |
                    | kv + history tables   |
                    +-----------------------+
```

## Standalone request path

1. The gRPC or HTTP adapter receives credentials or a query.
2. Both protocols use the same in-process `GraphService`, so they share users,
   roles, sessions, active-query tracking, and shutdown state.
3. The service validates the session, parses the statement, and recursively
   checks authorization, including compound and `PROFILE` statements.
4. The planner creates an executor backed by the same `KVStore`.
5. The executor reads or updates graph data, schemas, indexes, ontology state,
   and user records in redb.
6. The protocol adapter converts the resulting `DataSet` to protobuf or JSON.

The Graph service is therefore **not stateless** in the current runtime.
Non-root user records are durable and loaded into the authentication cache on
login, but sessions and active authentication state remain process-local.

## Main component boundaries

### Graph layer

`byoridb-graph` owns:

- authentication and built-in role checks;
- session lifecycle and selected-space tracking;
- query parsing/authorization orchestration;
- gRPC and HTTP adapters;
- active-query diagnostics, metrics, and graceful-drain integration.

The standalone binary creates one `Arc<GraphService>` and passes it to both
network servers. Embedded users can construct the service directly.

### Parser and executor

`byoridb-parser` converts the nGQL-inspired language into an AST.
`byoridb-executor` builds plans and implements DDL, DML, graph traversal,
pattern matching, indexes, ontology reasoning, recommendations, temporal
reads, `EXPLAIN`, and `PROFILE`.

The executor uses logical key namespaces in a `KVStore`. In standalone mode it
does not make a network hop to a separate Storage service.

### Storage and codecs

`byoridb-kvstore` supplies the storage abstraction and the production redb
implementation. `byoridb-codec` encodes new vertex and edge records with
protobuf and retains a JSON decode fallback for legacy records.

The redb file has two primary tables:

- `kv`: current data, schemas, indexes, users, and materialized state;
- `history`: append-only versions and tombstones for asserted vertex/edge
  point-in-time reads.

See [Storage engine](storage.html) for transaction and temporal details.

### Meta, partition, RPC, and Raft components

`byoridb-meta` and `byoridb-storage` contain Meta/Storage RPC services,
partition allocation and migration code, and a custom per-partition Raft
implementation. The distributed query executor can route selected operations
through Meta and Storage clients when explicitly constructed with those
clients.

Those pieces are not fully connected by `byoridb-server`: setting cluster peers
starts a Meta gRPC server, but does not bootstrap Storage/Raft peers or switch
the Graph executor to remote partition routing. They must not be interpreted as
a supported high-availability deployment. See
[Distributed systems](distributed.html).

## Data and consistency boundaries

- A redb write transaction is ACID and write transactions are serialized.
- Multi-row DML uses batched writes. The executor's `batch_apply` path commits
  its current-view entity mutations and matching history versions together in
  one redb transaction.
- There is no user-visible multi-statement transaction syntax. Compound
  statements execute sequentially and do not roll back earlier clauses after a
  later failure.
- Inference is materialized into the current view. Historical reads preserve
  asserted facts, not a historical inference closure.
- A clean shutdown first stops readiness, drains active queries, and then
  checkpoints redb.

## Deployment boundary

The repository provides a Docker Compose file and Azure AKS assets. Compose
starts independent standalone databases, while the AKS StatefulSet declares
one replica with a ReadWriteOnce volume. These files describe repository
deployment intent; they do not prove the state of any live environment.

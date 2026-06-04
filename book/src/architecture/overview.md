# Architecture Overview

ByoriDB uses a storage-compute separation architecture with three main services.

## System Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                         Clients                              │
│              (CLI, SDKs, HTTP, gRPC)                        │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                      Graph Service                           │
│  ┌─────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐    │
│  │ Parser  │→ │ Planner  │→ │ Executor │→ │ Results  │    │
│  └─────────┘  └──────────┘  └──────────┘  └──────────┘    │
└─────────────────────────────────────────────────────────────┘
           │                              │
           ▼                              ▼
┌─────────────────────┐      ┌─────────────────────────────────┐
│    Meta Service     │      │       Storage Service           │
│  ┌───────────────┐  │      │  ┌───────────┐  ┌───────────┐  │
│  │ Schema Cache  │  │      │  │  Part 1   │  │  Part 2   │  │
│  │ Space Config  │  │      │  │  (Raft)   │  │  (Raft)   │  │
│  │ User Auth     │  │      │  └───────────┘  └───────────┘  │
│  └───────────────┘  │      └─────────────────────────────────┘
└─────────────────────┘                    │
           │                               ▼
           ▼                      ┌─────────────────┐
    ┌──────────────┐              │    KVStore      │
    │   KVStore    │              │    (redb)       │
    │    (redb)    │              └─────────────────┘
    └──────────────┘
```

## Components

### Graph Service

The stateless query engine responsible for:

- **Query Parsing**: nGQL → AST using `byoridb-parser`
- **Query Planning**: AST → Execution Plan
- **Query Execution**: Coordinates with Meta and Storage services
- **Result Aggregation**: Combines results from partitions

Key characteristics:
- Horizontally scalable (stateless)
- gRPC and HTTP endpoints
- Connection pooling to downstream services

### Meta Service

Manages all metadata:

- **Spaces**: Logical databases
- **Schemas**: Tags, Edges, Indexes
- **Partitions**: Data distribution mapping
- **Users**: Authentication and authorization
- **Schema Versions**: For online schema changes

Key characteristics:
- Single leader (can be replicated via Raft)
- In-memory schema cache with TTL
- Persistent storage in KVStore

### Storage Service

Stores and retrieves graph data:

- **Vertices**: Tag data keyed by VID
- **Edges**: Edge data keyed by (src, edge_type, rank, dst)
- **Partitioning**: VID-based consistent hashing
- **Replication**: Multi-replica via Raft consensus

Key characteristics:
- Horizontally partitioned by VID
- Each partition has its own Raft group
- Supports predicate pushdown

### KVStore

Underlying storage engine:

- **redb**: pure-Rust embedded B-tree storage
- **Raft**: Distributed consensus protocol
- **Snapshots**: Point-in-time backups
- **Compaction**: Background optimization

## Data Flow

### Write Path

```
1. Client → Graph Service (INSERT VERTEX)
2. Graph Service → Meta Service (get partition info)
3. Graph Service → Storage Service (write to leader)
4. Storage Leader → Raft Log (replicate)
5. Storage Leader → redb (apply)
6. Ack back to client
```

### Read Path

```
1. Client → Graph Service (FETCH PROP)
2. Graph Service → Meta Service (get schema + partition)
3. Graph Service → Storage Service (read from any replica)
4. Storage → redb → Return data
5. Graph Service → Apply schema version transformation
6. Return to client
```

## Crate Dependencies

```
byoridb-client
    └── byoridb
            ├── byoridb-executor
            │       └── byoridb-parser
            ├── byoridb-meta
            │       └── byoridb-kvstore
            └── byoridb-storage
                    ├── byoridb-codec
                    └── byoridb-kvstore
                            └── byoridb-common
```

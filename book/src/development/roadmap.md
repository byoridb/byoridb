# Roadmap

Current status and future plans for ByoriDB.

## Current Version (v0.1)

### Core Features

- [x] nGQL Parser
  - [x] DDL statements (CREATE/DROP SPACE/TAG/EDGE)
  - [x] DML statements (INSERT/UPDATE/DELETE VERTEX/EDGE)
  - [x] DQL statements (FETCH, GO, MATCH, LOOKUP, FIND PATH)
  - [x] ALTER TAG/EDGE ADD (online schema change)

- [x] Storage Engine
  - [x] Pure-Rust KV (redb) integration
  - [x] Vertex/Edge encoding
  - [x] Schema version support
  - [x] Bloom filter
  - [x] Block cache

- [x] Distributed System
  - [x] Raft consensus
  - [x] Leader election
  - [x] Log replication
  - [x] Snapshots

- [x] Meta Service
  - [x] Space management
  - [x] Schema management
  - [x] Schema versioning (lazy migration)
  - [x] User authentication

## Upcoming (v0.2)

### Query Enhancements

- [ ] Subqueries
- [ ] Common Table Expressions (WITH)
- [ ] Window functions
- [ ] Full-text search

### Schema Operations

- [ ] ALTER TAG/EDGE DROP column
- [ ] ALTER TAG/EDGE MODIFY column
- [ ] Online index creation

### Performance

- [ ] Query plan caching
- [ ] Parallel query execution
- [ ] Vectorized execution
- [ ] Cost-based optimizer

### Operations

- [ ] Online backup
- [ ] Point-in-time recovery
- [ ] Cluster rebalancing

## Future (v0.3+)

### Advanced Features

- [ ] Graph algorithms (PageRank, shortest path, etc.)
- [ ] Temporal graphs
- [ ] Geospatial support
- [ ] Graph neural network integration

### Enterprise Features

- [ ] Multi-tenancy
- [ ] Role-based access control
- [ ] Audit logging
- [ ] Encryption at rest

### Ecosystem

- [ ] Python client
- [ ] Java client
- [ ] JavaScript client
- [ ] JDBC driver
- [ ] Spark connector

### Cloud Native

- [ ] Kubernetes operator
- [ ] Helm charts
- [ ] Auto-scaling
- [ ] Multi-region replication

## Contributing

Want to contribute to a feature? Check out our [Contributing Guide](./contributing.md).

## Feature Requests

Have an idea? Open an issue with the `enhancement` label.

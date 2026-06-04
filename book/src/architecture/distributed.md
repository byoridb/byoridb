# Distributed System

ByoriDB is designed for distributed deployment with high availability.

## Raft Consensus

Each partition uses the Raft protocol for:

- **Leader Election**: Automatic failover when leader fails
- **Log Replication**: All writes go through leader and replicate to followers
- **Consistency**: Strong consistency within each partition

### Raft States

```
┌─────────┐  timeout   ┌───────────┐  majority vote  ┌────────┐
│Follower │──────────→ │ Candidate │───────────────→ │ Leader │
└─────────┘            └───────────┘                 └────────┘
     ↑                       │                            │
     │        split vote     │                            │
     └───────────────────────┘                            │
     │                                     heartbeat      │
     └────────────────────────────────────────────────────┘
```

### Configuration

```toml
[raft]
election_timeout_ms = 1000     # Time before new election
heartbeat_interval_ms = 100    # Leader heartbeat frequency
snapshot_interval = 10000      # Entries before snapshot
max_log_entries = 100000       # Max log entries to keep
```

## Partitioning

Data is distributed across partitions using VID-based hashing:

```
partition_id = hash(vid) % num_partitions
```

### Partition Configuration

```sql
CREATE SPACE my_space(
    vid_type = INT64,
    partition_num = 10,
    replica_factor = 3
);
```

| Parameter | Description | Recommendation |
|-----------|-------------|----------------|
| `partition_num` | Number of partitions | 10-100 per machine |
| `replica_factor` | Copies of each partition | 3 for production |

### Partition Distribution

```
┌──────────────────────────────────────────────────────────┐
│                    Storage Cluster                        │
│                                                          │
│  Node 1          Node 2          Node 3                  │
│  ┌────┐          ┌────┐          ┌────┐                 │
│  │P1-L│          │P1-F│          │P1-F│   Partition 1   │
│  ├────┤          ├────┤          ├────┤                 │
│  │P2-F│          │P2-L│          │P2-F│   Partition 2   │
│  ├────┤          ├────┤          ├────┤                 │
│  │P3-F│          │P3-F│          │P3-L│   Partition 3   │
│  └────┘          └────┘          └────┘                 │
│                                                          │
│  L = Leader, F = Follower                               │
└──────────────────────────────────────────────────────────┘
```

## Replication

### Write Replication

1. Client sends write to Graph Service
2. Graph Service routes to partition leader
3. Leader appends to Raft log
4. Leader replicates to followers
5. Once majority acknowledges, commit
6. Apply to redb and respond

### Read Options

| Mode | Consistency | Performance |
|------|-------------|-------------|
| Leader | Strong | Lower |
| Follower | Eventual | Higher |

## Failure Handling

### Leader Failure

1. Followers detect missing heartbeats
2. Election timeout triggers new election
3. New leader elected (requires majority)
4. Service resumes with new leader

**Recovery time:** ~2-3 election timeouts

### Follower Failure

1. Leader detects follower is down
2. Continues serving with remaining replicas
3. When follower recovers, catches up via log
4. If too far behind, snapshot transfer

### Network Partition

- Partition with majority continues serving
- Minority partition becomes read-only
- Automatic recovery when network heals

## Cluster Management

### Adding Nodes

```bash
# Start new storage node
byoridb-storage --join cluster-addr:port
```

The cluster automatically:
1. Assigns partitions to new node
2. Rebalances data
3. Updates partition map

### Removing Nodes

```bash
# Graceful removal
byoridb-admin node remove <node-id>
```

1. Migrate partitions to other nodes
2. Wait for replication to complete
3. Remove from cluster

### Monitoring

Key metrics to monitor:

- `raft_leader_changes` - Leadership changes
- `raft_log_entries` - Log size
- `partition_status` - Per-partition health
- `replication_lag` - Follower delay

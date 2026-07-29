# Distributed systems

[한국어](../ko/architecture/distributed.html)

> **Status: component implementation, not a supported cluster deployment.**
> Run ByoriDB as a single server unless you are developing and validating the
> unfinished distributed path itself.

ByoriDB contains substantial distributed-system building blocks, but the
presence of those modules does not mean the `byoridb-server` binary currently
forms a replicated cluster.

## Components in the repository

### Partition routing

Spaces can carry `partition_num` and `replica_factor` metadata. The common hash
function maps a VID to a one-based partition, and the distributed executor can
group vertex and edge requests by partition. Edge requests are partitioned by
source VID.

Meta components maintain space, host, and partition-allocation records. The
distributed query executor can consult a `MetaClient`, select a Storage host,
issue parallel RPCs, and aggregate selected `FETCH`, edge, scan, and index
operations.

### Storage RPC

`byoridb-storage` defines protobuf services for vertex/edge access, scans,
indexes, partition migration, and Raft transport. `byoridb-meta` also contains
migration and rebalance helpers.

These services and clients are library components. The default launcher does
not start and connect a complete set of remote Storage services.

### Custom Raft

`byoridb-storage/src/raft/` implements a custom Raft state machine and transport
with:

- follower, candidate, and leader states;
- request-vote and append-entries handling;
- persistent term/vote/log state;
- chunked snapshot installation;
- per-`(space_id, part_id)` group management;
- configuration-change commands and a gRPC network driver.

The code has unit and component tests, but it has not been externally validated
as a production consensus implementation. It must not be used to claim data
replication or failover from the current server deployment.

## What the launcher currently does

`byoridb-server` reads the following cluster settings:

```text
BYORIDB__CLUSTER__NODE_ID
BYORIDB__CLUSTER__PEERS
BYORIDB__CLUSTER__ADVERTISE_ADDR
BYORIDB__CLUSTER__BOOTSTRAP
BYORIDB__CLUSTER__META_ADDR
```

When `BYORIDB__CLUSTER__PEERS` is empty, the server runs the normal standalone
path. When it is non-empty, the launcher additionally starts a Meta gRPC
server. It does **not** currently:

- bootstrap Storage/Raft peers from that list;
- start the Storage query/Raft RPC topology required by all partitions;
- construct the Graph execution context with remote Meta and Storage clients;
- route normal Graph queries through the distributed executor;
- implement a complete membership/bootstrap lifecycle;
- make authentication sessions shared or restart-durable.

`BYORIDB__CLUSTER__BOOTSTRAP` is parsed but is not yet connected to a complete
bootstrap sequence.

## Deployment files are standalone

The checked-in deployment assets deliberately do not establish a cluster:

- `docker-compose.yml` starts three independent `byoridb-server` processes,
  each with its own volume and no cluster environment. Writes are not
  replicated between them.
- `deploy/azure/k8s/03-statefulset.yaml` declares one replica and one
  ReadWriteOnce PVC.

Changing either replica count without completing distributed storage and
session routing can produce isolated databases and inconsistent client
behavior. Do not place independent replicas behind one load balancer as though
they shared data.

Repository manifests describe desired configuration only. This page makes no
claim about the current state of a live Kubernetes or Azure environment.

## Session and authorization constraint

Non-root user records are persisted in redb and loaded into an in-process auth
cache. Session IDs, selected spaces, role snapshots, and active-query state are
process-local. A future multi-Graph deployment therefore also needs a defined
session-affinity or shared-session design and cluster-wide revocation behavior.

## Work required for a supported cluster

A production-ready distributed mode requires at least:

1. launcher wiring for Storage RPC, Raft drivers, peer discovery, and group
   bootstrap;
2. Graph contexts connected to Meta/Storage clients for every supported query
   path, with explicit local/distributed parity;
3. tested membership changes, leader redirects, recovery, snapshots, and data
   migration;
4. authentication/session behavior across Graph replicas;
5. Docker/Kubernetes topology and end-to-end multi-process tests that verify
   replication and failover rather than only component routing;
6. operational runbooks, upgrade compatibility, observability, and fault
   injection testing.

Until those gates close, `partition_num` and `replica_factor` should be treated
as schema/component metadata, not evidence that the standalone server has
created physical replicas.

[한국어](../../ko/guide/ngql/spaces.html)

# Spaces

A space is a logical namespace for graph schema and data. Select a space with
`USE` before creating tags or edges or reading and writing graph records.

## Create a space

Spaces support either integer or fixed-length string VIDs:

```sql
CREATE SPACE social;
CREATE SPACE IF NOT EXISTS social (vid_type = INT64);
CREATE SPACE accounts (vid_type = FIXED_STRING(32));
```

The current defaults are `partition_num = 100`, `replica_factor = 1`,
`vid_type = INT64`, and hash partition metadata. Options may be specified
explicitly:

```sql
CREATE SPACE analytics (
    partition_num = 16,
    replica_factor = 1,
    vid_type = INT64
) PARTITION BY HASH;
```

The parser also accepts `PARTITION BY MODULO` and
`PARTITION BY RANGE(100, 200, 300)`. In standalone mode these fields are stored
as metadata; they do not turn one process into a multi-node cluster.

`FIXED_STRING(N)` accepts quoted UTF-8 identifiers whose encoded length is at
most `N` bytes. A space uses one VID type consistently, so integer literals are
normally rejected in a `FIXED_STRING` space and string literals are rejected in
an `INT64` space:

```sql
USE accounts;
CREATE TAG account(name STRING);
INSERT VERTEX account(name) VALUES "acct-001":("Primary");
FETCH PROP ON account "acct-001";
```

Internally, a `FIXED_STRING` space stores a persistent bidirectional mapping
between each UTF-8 VID and a stable **negative** `i64` surrogate. New mappings
never use a non-negative value. The records use
`{space}:vid-map:{hex-utf8}` for string to surrogate, with the signed `i64`
stored as exactly eight big-endian bytes. The reverse record uses
`{space}:vid-rev:{surrogate}` with the original UTF-8 bytes as its value. They
are not recycled when graph data is deleted and are removed with the space's
normal key prefix when `DROP SPACE` runs.

This mapping preserves the integer contract tracked by
[issue #49](https://github.com/byoridb/byoridb/issues/49): vertex, edge,
tag-to-VID, index, partition, codec, and storage keys continue to carry an
`i64`, and storage RPC/protobuf messages continue to use their existing integer
VID fields. Query and API inputs/results translate only at the executor
boundary.

Mapping-backed `FIXED_STRING` execution is supported in standalone mode only.
The mapping records do not yet have cluster-wide ownership, replication,
consensus, or routing, so a distributed or multi-coordinator deployment must
use `INT64` spaces. `RECOMMEND` is also currently INT64-only; see
[Data queries](./dql.md#recommend).

### Legacy integer bridge

A space created before durable string mappings may contain live graph records
under non-negative integer VIDs. ByoriDB exposes only an **actual live,
unmapped, non-negative** VID through a temporary read/delete compatibility
bridge. It is returned as an integer and emits an operator warning. Such a
space is write-frozen for those legacy IDs: INSERT, UPDATE, and new edge writes
with integer endpoints are rejected, and a raw negative internal surrogate is
always rejected.

Do not use this bridge as a mixed-VID operating mode. Export the legacy graph,
recreate the `FIXED_STRING` space, and import it with quoted string VIDs. An
unmapped integer with no live vertex or incident-edge evidence remains a normal
point miss.

## Select a space

```sql
USE social;
```

The selected space belongs to the authenticated session. A later request on the
same session uses that space until another `USE` succeeds.

## Inspect spaces

```sql
SHOW SPACES;
DESCRIBE SPACE social;
```

`SHOW SPACES` reports each stored space's ID and configuration metadata.

## Drop a space

```sql
DROP SPACE social;
DROP SPACE IF EXISTS social;
```

`DROP SPACE` is destructive: it removes the space's schema, current graph data,
and associated index data. The current prefix-deletion path does not purge the
separate temporal-history table, so `DROP SPACE` is not a secure erasure of all
historical bytes. Do not use it as a way to leave a session; select a different
space with `USE` instead.

## Authorization

- `USE`, `SHOW`, and `DESCRIBE` require read access.
- `CREATE SPACE` requires create access on the new space name.
- `DROP SPACE` requires drop access.

All built-in role permissions currently use the wildcard space `*`. ByoriDB has
no user-facing space-scoped `GRANT` syntax yet, so a built-in role applies across
every space.

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

The bridge is a **transition, not a steady state**: it exists so an upgraded
space stays readable long enough to be migrated, and it warns once per space
when used. Mixing the two namespaces is not a supported operating mode. An
unmapped integer with no live vertex or incident-edge evidence remains a normal
point miss.

### Migrating from client-hashed integer VIDs

Before `FIXED_STRING` spaces existed, a client keying entities by name had to
hash the name into an `i64` itself. Such a space is an ordinary `INT64` space,
so the legacy bridge above does not apply to it — the bridge is for a space
whose descriptor already says `FIXED_STRING`.

**A space's VID type is fixed when it is created.** `ALTER` has `TAG`, `EDGE`,
and `USER` forms only; there is no `ALTER SPACE`, and `vid_type` comes from the
space descriptor written by `CREATE SPACE`. So migration is a copy into a new
space, never a conversion in place:

```sql
-- 1. Create the destination. N must fit your longest identifier in bytes.
CREATE SPACE memory_v2 (vid_type = FIXED_STRING(128));
USE memory_v2;

-- 2. Recreate the schema. Tags, edges, indexes, classes, and shapes do not
--    come along with the data.
CREATE TAG note(name STRING, body STRING);
CREATE EDGE rel(kind STRING);

-- 3. Read each vertex and edge from the old space and insert it here under its
--    string identifier, which is the name the client was hashing.
INSERT VERTEX note(name, body) VALUES "decision:use-redb":("decision:use-redb", "adopt redb");
INSERT EDGE rel(kind) VALUES "decision:use-redb"->"module:kvstore":("affects");
```

Reads then return the string, and traversal works from it:

```sql
MATCH (n:note) WHERE id(n) == "decision:use-redb" RETURN id(n) AS vid;
GO FROM "decision:use-redb" OVER rel YIELD rel.kind AS kind;
```

Writing an integer VID into the new space is refused rather than silently
creating a second identity for the same entity:

```
INSERT VERTEX note(name, body) VALUES 111:("x", "y");
[ERROR] space 'memory_v2' uses FIXED_STRING VIDs; integer VID 111 is
        read/delete-only legacy data and cannot be written
```

The internal surrogates are not addressable either. On a write the integer rule
above applies to every integer, negative included; on a read or delete, a raw
negative VID is refused as an internal surrogate.

#### History does not follow a migration

This is the consequence to plan around. History is keyed by space and VID, so
re-inserting an entity under a new identifier creates a **new** history that
begins at the insert:

```sql
-- In the new space, before the insert: no rows.
FETCH PROP ON note "decision:use-redb" AS OF 1785283200000;

-- In the old space, the pre-migration state is still there, under the old VID.
USE memory_v1;
FETCH PROP ON note 111 AS OF 1785283200000;
```

So a migration is not a rename — it is a re-assertion of current facts. Keep the
old space as the archive of record if past states matter, and do not `DROP` it
expecting `AS OF` to keep working in the new one. There is no supported way to
carry history across a re-key.

#### Before you migrate

- `RECOMMEND` is INT64-only and is refused in a `FIXED_STRING` space, naming the
  space. If a workload depends on it, keep that data in an `INT64` space; see
  [Data queries](./dql.md#recommend).
- Mapping-backed `FIXED_STRING` execution is standalone-only, as above.
- Identifiers are limited to `N` **bytes**, not characters, so non-ASCII names
  need headroom.

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

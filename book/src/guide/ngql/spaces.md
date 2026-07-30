[한국어](../../ko/guide/ngql/spaces.html)

# Spaces

A space is a logical namespace for graph schema and data. Select a space with
`USE` before creating tags or edges or reading and writing graph records.

## Create a space

The reliable standalone form uses integer VIDs:

```sql
CREATE SPACE social;
CREATE SPACE IF NOT EXISTS social (vid_type = INT64);
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

Although `FIXED_STRING(N)` is accepted as a space VID-type option, current DML
planning requires integer literal VIDs. Use `INT64` until string-VID execution is
implemented.

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

[한국어](../ko/guide/ngql-syntax.html)

# nGQL syntax

ByoriDB implements an nGQL-compatible graph query language. It is a focused
subset with ByoriDB extensions; syntax from another nGQL implementation is not
automatically supported.

## Statement families

| Family | Current statements |
| --- | --- |
| Spaces and schema | `CREATE`, `ALTER`, `DROP`, `SHOW`, `DESCRIBE`, `USE` |
| Data mutation | `INSERT VERTEX`, `UPDATE VERTEX`, `DELETE VERTEX`, `INSERT EDGE`, `DELETE EDGE` |
| Query | `FETCH PROP`, `GO`, `MATCH`, `LOOKUP`, `FIND`, `RECOMMEND` |
| Ontology | `CREATE CLASS`, `CREATE SHAPE`, `CHECK CONSISTENCY`, `CHECK SHAPE`, `WHY` |
| Administration | `CREATE/ALTER/DROP USER`, `GRANT/REVOKE ROLE`, `SHOW SESSIONS`, `BALANCE` |
| Inspection | `EXPLAIN <statement>`, `PROFILE <statement>` |

See the linked guide pages for execution limitations. For example,
`UPDATE EDGE` is accepted by the parser but is not wired to a working executor,
and `LOOKUP` currently operates on tags rather than edge types.

## Lexical rules

- Keywords are case-insensitive: `CREATE` and `create` are equivalent.
- User-defined identifiers retain their spelling and are case-sensitive in
  stored schema and user lookups.
- Both single-quoted and double-quoted string literals are accepted.
- `--` starts a line comment.
- A trailing semicolon is optional.

Multiple semicolon-separated statements sent in one request form a compound
statement and run in order. There is no transaction-control syntax, so separate
requests do not form a transaction.

```sql
CREATE SPACE demo; USE demo; SHOW TAGS;
```

Compound statements may bind a result for a later clause:

```sql
$friends = GO FROM 1 OVER follows YIELD dst(edge) AS vid;
FETCH PROP ON person $friends.vid;
```

This is the supported composition mechanism; a `|` pipeline operator is not
implemented.

## Property types

The property-schema parser currently accepts:

| Type | Notes |
| --- | --- |
| `BOOL` | Boolean |
| `INT8`, `INT16`, `INT32`, `INT64` | Signed integers; `INT` is an alias for `INT64` |
| `FLOAT`, `DOUBLE` | Floating-point numbers |
| `STRING` | Text |
| `TIMESTAMP` | Integer epoch value or accepted temporal string |
| `DATE`, `TIME`, `DATETIME` | Temporal value represented by an accepted string or integer where supported |

The AST contains additional type variants, but `FIXED_STRING` and `GEOGRAPHY`
are not currently accepted as tag/edge property declarations by the parser.
Do not document or deploy against those variants until their parser and
execution paths are complete.

## Vertex IDs

Current DML and query planning requires integer vertex IDs. Use the default
`INT64` VID type:

```sql
CREATE SPACE demo (vid_type = INT64);
```

`FIXED_STRING(N)` can be parsed as space metadata, but current INSERT/FETCH/GO
planning still rejects string VIDs. It is not an operational string-VID mode.

## Expressions

Predicates support comparison and boolean operators such as `==`, `!=`, `<`,
`<=`, `>`, `>=`, `AND`, and `OR`. `MATCH` supports functions and aggregates
including `id`, `count`, `sum`, `avg`, `min`, and `max` in the execution paths
described in the query guide.

Mutation assignment values are more restricted than query expressions. In
particular, `UPDATE VERTEX ... SET` currently plans literal and list values, not
arithmetic expressions such as `score = score + 1`.

## Authentication and authorization

Authentication happens before statements are executed. User and role
administration, `SHOW SESSIONS`, and `BALANCE` require a `GOD` or `ADMIN`
session. Other statements are checked against the authenticated session's
built-in roles, including every clause inside compound statements and the inner
statement executed by `PROFILE`.

The built-in roles currently apply to all spaces. There is no nGQL syntax for a
space-scoped grant, so these roles are not a tenant-isolation mechanism.

## Guide pages

- [Spaces](./ngql/spaces.md)
- [Schema](./ngql/schema.md)
- [Data mutation](./ngql/dml.md)
- [Data queries](./ngql/dql.md)
- [Users and roles](./ngql/users.md)

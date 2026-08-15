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
| Administration | `CREATE/ALTER/DROP USER`, `GRANT/REVOKE ROLE`, `SHOW USER/USERS`, `SHOW ROLE/ROLES`, `SHOW SESSIONS`, `BALANCE` |
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

Choose one VID type per space. `INT64` uses integer literals and
`FIXED_STRING(N)` uses quoted UTF-8 strings of at most `N` bytes:

```sql
CREATE SPACE demo (vid_type = INT64);
CREATE SPACE accounts (vid_type = FIXED_STRING(32));
```

String VIDs are supported in standalone graph CRUD, FETCH, GO, FIND, LOOKUP,
and MATCH. Mapping-backed `FIXED_STRING` is not supported in distributed or
multi-coordinator execution, and `RECOMMEND` remains INT64-only. Except for the
read/delete-only live legacy bridge described in the
[space guide](./ngql/spaces.md#legacy-integer-bridge), mixing an integer literal
into a `FIXED_STRING` space, or a string literal into an `INT64` space, is
rejected.

## Expressions

Predicates support comparison and boolean operators such as `==`, `!=`, `<`,
`<=`, `>`, `>=`, `AND`, and `OR`. `MATCH` supports functions and aggregates
including `id`, `count`, `sum`, `avg`, `min`, and `max` in the execution paths
described in the query guide.

### Set membership

`IN` and `NOT IN` test a value against a list literal, which is the compact form
of an `OR` chain over equality:

```ngql
MATCH (p:person) WHERE id(p) IN [1, 2, 3] RETURN p;
MATCH (p:person) WHERE p.person.name NOT IN ['ada', 'grace'] RETURN p;
LOOKUP ON person WHERE person.name IN ['ada', 'grace'] YIELD person.name;
```

The right operand must be a list literal. Semantics:

- An empty list matches nothing, so `x IN []` is false and `x NOT IN []` is true.
- `NULL IN <list>` is unknown, and a `NULL` element makes a non-match unknown
  rather than false: `2 IN [1, NULL]` is unknown, while `1 IN [1, NULL]` is
  still true because a match outranks an unknown.
- A `WHERE` clause treats unknown as not matching, so an unknown row is filtered
  out rather than raising an error.
- A right operand that is not a list is false rather than an error, the same way
  the string operators treat a type mismatch.

`IN` is not an index-driven lookup. It is evaluated as a filter, so it narrows
results without changing how the rows are reached.

Note that `LOOKUP` compares values strictly, while `MATCH` compares numbers
across types. So `p.person.age IN [36.0]` matches an integer `36` under `MATCH`
but not under `LOOKUP`. That asymmetry predates `IN` and applies to `==` as well.

`in` remains usable as an ordinary property or alias name.

Mutation assignment values are more restricted than query expressions. In
particular, `UPDATE VERTEX ... SET` currently plans literal and list values, not
arithmetic expressions such as `score = score + 1`.

## Authentication and authorization

Authentication happens before statements are executed. User and role
administration, `SHOW USER`, `SHOW USERS`, `SHOW ROLE`, `SHOW ROLES`,
`SHOW SESSIONS`, and `BALANCE` require a `GOD` or `ADMIN` session. Other
statements are checked against the authenticated session's built-in roles,
including every clause inside compound statements and the inner statement
executed by `PROFILE`.

The built-in roles currently apply to all spaces. There is no nGQL syntax for a
space-scoped grant, so these roles are not a tenant-isolation mechanism.

## Guide pages

- [Spaces](./ngql/spaces.md)
- [Schema](./ngql/schema.md)
- [Data mutation](./ngql/dml.md)
- [Data queries](./ngql/dql.md)
- [Users and roles](./ngql/users.md)

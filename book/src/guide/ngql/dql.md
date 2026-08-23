[한국어](../../ko/guide/ngql/dql.html)

# Data queries

Query statements use the space selected by the authenticated session.

## FETCH PROP

Fetch vertices by the VID type selected for the space:

```sql
FETCH PROP ON person 1;
FETCH PROP ON person 1, 2, 3;
FETCH PROP ON * 1;
FETCH PROP ON person "alice";
```

The multi-VID form uses one internal `batch_get`, but the repository does not
yet claim an LDBC-scale limit or response-size target for batches of hundreds
of IDs. Keep HTTP requests below the 1 MiB query limit and measure the response
size for your workload. Large-batch correctness and performance remain tracked
in [issue #10](https://github.com/byoridb/byoridb/issues/10).

Fetch edges by endpoint pair:

```sql
FETCH PROP ON knows 1->2;
FETCH PROP ON * 1->2;
FETCH PROP ON knows "alice"->"bob";
```

The current temporal read surface accepts an epoch-millisecond timestamp for
both vertices and edges:

```sql
FETCH PROP ON person 1 AS OF 1785283200000;
FETCH PROP ON knows 1->2 AS OF 1785283200000;
```

`AS OF` resolves asserted history using the same timestamp for valid time and
transaction time. Historical MATCH, GO, and RECOMMEND are not implemented, and
past inferred facts are not guaranteed to be reconstructed.

## GO

Traverse one or more outgoing edge types:

```sql
GO FROM 1 OVER knows;
GO FROM 1 OVER knows, follows;
GO FROM 1 OVER *;
```

Use exact or ranged hop counts and a direction:

```sql
GO 2 STEPS FROM 1 OVER knows;
GO 1..3 STEPS FROM 1 OVER knows;
GO FROM 1 OVER knows REVERSELY;
GO FROM 1 OVER knows BIDIRECT;
```

Filter and project edge or destination properties:

```sql
GO FROM 1 OVER knows
WHERE knows.since >= 2020
YIELD src(edge) AS src, dst(edge) AS dst, knows.since AS since;

GO FROM 1 OVER knows
YIELD $$.person.name AS friend_name;
```

Destination-property projection deduplicates destination VIDs and loads them
with one internal `batch_get`. `EXPLAIN` and `PROFILE` expose this operation as
`GetVertices`. The engine portion of
[issue #10](https://github.com/byoridb/byoridb/issues/10) is covered; conversion
of the external LDBC Q9 harness and its `<10s` acceptance measurement remain.

The configured execution guard rejects GO ranges above 20 steps by default.

## MATCH

Match vertex and edge patterns:

```sql
MATCH (p:person) RETURN id(p) AS vid, p.person.name AS name;

MATCH (a:person)-[e:knows]->(b:person)
WHERE a.person.name == "Alice" AND b.person.age >= 20
RETURN b.person.name AS friend;
```

Literal property maps and variable-length edges are supported:

```sql
MATCH (p:person {name: "Alice"}) RETURN p;
MATCH (a:person)-[:knows*1..3]->(b:person) RETURN id(b) AS vid;
```

Multiple comma-separated patterns join on shared variables. `OPTIONAL MATCH`,
`GROUP BY`, `ORDER BY`, `LIMIT`, and `OFFSET` are also available in the current
MATCH path.

An edge variable preserves stored edge identity. Use `src(e)`, `dst(e)`,
`type(e)`, `rank(e)` (or `ranking(e)`), `properties(e)`, or return `e` itself.
`src(e)` and `dst(e)` preserve the stored orientation even when the pattern is
incoming or undirected. Passing a bound vertex instead of an edge returns a
typed bad-type null; an unknown variable returns a typed unknown-property null.

Aggregates include `COUNT`, `SUM`, `AVG`, `MIN`, and `MAX`:

```sql
MATCH (p:person)
RETURN p.person.city AS city, COUNT(*) AS people
GROUP BY p.person.city
ORDER BY people DESC
LIMIT 10;
```

For ontology-aware membership, use `is_a`:

```sql
MATCH (n:dog) WHERE is_a(n, "animal") RETURN id(n) AS vid;
```

## Functions

Calling a function the engine does not implement is a **query error naming the
function**, not a `NULL`. That matters most in `WHERE`: a null predicate is
false for every row, so an unsupported function would report zero results and be
indistinguishable from "nothing matched".

```
nebula> MATCH (n:doc) WHERE frobnicate(n.doc.body) RETURN n;
[ERROR] Unknown function: frobnicate
```

### Graph functions

Available inside `MATCH`, where they read graph state rather than argument
values:

| Function | Returns |
| --- | --- |
| `id(v)` | the VID of a bound vertex |
| `src(e)`, `dst(e)` | edge endpoints, in stored orientation |
| `type(e)` | edge type name |
| `rank(e)`, `ranking(e)` | edge rank |
| `properties(v)`, `properties(e)` | flat map of all properties |
| `tags(v)`, `labels(v)` | the vertex's tag names |
| `is_a(v, "Class")` | class membership, including `SUBCLASS OF` ancestors |

### Aggregates

`COUNT`, `SUM`, `AVG`, `MIN`, `MAX`, `COLLECT`.

### Scalar functions

Available wherever an expression is evaluated:

| Group | Functions |
| --- | --- |
| Case | `lower` / `toLower`, `upper` / `toUpper` |
| Size | `length` / `size` |
| Text | `contains`, `starts_with` / `startsWith`, `ends_with` / `endsWith` |
| Numeric | `abs`, `floor`, `ceil`, `round` |
| Null | `is_null` / `isNull`, `is_not_null` / `isNotNull`, `coalesce` |

Names are case-insensitive, so `toLower`, `TOLOWER`, and `tolower` are the same
function. A supported function given an argument of the wrong type yields a
typed null rather than failing the query; only an unknown *name* is refused.

### Case-sensitive text matching

`CONTAINS`, `STARTS WITH`, and `ENDS WITH` compare **exactly**, so
`CONTAINS 'worktrees'` does not match `Worktrees`. Fold both sides instead of
issuing one query per spelling:

```sql
MATCH (n:doc) WHERE toLower(n.doc.body) CONTAINS 'worktrees'
RETURN n.doc.body AS body;
```

There is no case-insensitive comparison operator; folding with `toLower` is the
supported approach.

## LOOKUP

Current `LOOKUP` targets tags:

```sql
LOOKUP ON person WHERE person.name == "Alice";
LOOKUP ON person WHERE person.age >= 21 YIELD person.name, person.age LIMIT 20;
```

An equality predicate on an indexed tag property can use the secondary index.
Range predicates (`>`, `>=`, `<`, and `<=`) currently fall back to a bounded
full scan even when that property has an index; index range scans are tracked
in [issue #1](https://github.com/byoridb/byoridb/issues/1). The default fallback
scan limit is 100,000 rows and is configurable. `LOOKUP` on an edge type
currently returns an error rather than performing an edge lookup. Use
`EXPLAIN` or `PROFILE` to verify the selected access path.

## FIND paths

```sql
FIND SHORTEST PATH FROM 1 TO 3 OVER knows;
FIND SHORTEST PATH FROM 1 TO 3 OVER road WEIGHT BY distance;
FIND SHORTEST PATH FROM 1 TO 3 OVER knows BIDIRECT UPTO 5 STEPS;
FIND ALL SHORTEST PATHS FROM 1 TO 3 OVER knows UPTO 5 STEPS;
```

The `OVER` target is one edge type or `*`, not a comma-separated edge list.
Path traversal and all-shortest-path enumeration are bounded by executor
resource caps.

## RECOMMEND

Structural mode ranks candidates by Jaccard overlap of outgoing neighbors:

```sql
RECOMMEND SIMILAR TO 1001 OVER has_brand, in_category LIMIT 5;
RECOMMEND SIMILAR TO 1001 OVER * WHERE channel != seed.channel LIMIT 5;
```

Embedding mode uses cosine similarity over a numeric-list property:

```sql
RECOMMEND SIMILAR TO 1001 BY EMBEDDING embedding LIMIT 5;
```

Blend both signals with query-time weights:

```sql
RECOMMEND SIMILAR TO 1001
BLEND EMBEDDING embedding 0.7 OVER has_brand, in_category 0.3
LIMIT 5;
```

The default recommendation limit is 10. Small vector collections use an exact
scan; above the implementation threshold, a persisted HNSW index is used and
maintained by current mutation paths.

`RECOMMEND` currently accepts an integer seed and returns integer VIDs, so it is
supported only in `INT64` spaces. Do not run it in a `FIXED_STRING` space.

## Compound queries and inspection

Bind a result to a variable and consume its VID column in a later clause:

```sql
$first = GO FROM 1 OVER knows YIELD dst(edge) AS vid;
GO FROM $first.vid OVER knows YIELD dst(edge) AS vid;
```

Send the entire compound query as one request. There is no `|` pipeline syntax.

Use `EXPLAIN` to inspect a logical plan without executing it and `PROFILE` to
execute it with operator metrics:

```sql
EXPLAIN MATCH (p:person) RETURN p;
PROFILE GO FROM 1 OVER knows;
```

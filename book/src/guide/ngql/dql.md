[한국어](../../ko/guide/ngql/dql.html)

# Data queries

Query statements use the space selected by the authenticated session. This page
describes the current source; consult the release matching your deployed binary.

## Inference status and recovery

```sql
USE my_space;
CHECK INFERENCE;
REPAIR INFERENCE;
```

`CHECK INFERENCE` returns `space` and `status` columns, with `complete` or
`repair_required`; ordinary read permission is sufficient. `REPAIR INFERENCE`
requires GOD/ADMIN and a writable request, rebuilds current closure and returns
the same status shape. It is repeatable and preserves asserted history.

A semantic mutation commits its asserted changes and a durable incomplete marker
before completing inference. A failure, cancellation or restart leaves graph
reads and further mutations blocked until repair succeeds. Status checks remain
available. An error does not prove that assertions rolled back: have an
administrator repair, inspect current facts, and only then decide whether to retry.
Compound statements commit clause by clause; they are not a transaction.

Updating an inferred-only edge explicitly promotes it to an assertion. Deleting
one directly is rejected; delete its supporting assertions instead. The
`__inferred__` property is reserved for the engine. Historical inference is not
reconstructed by repair, and sameAs merges remain irreversible.

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

Fetch edges by endpoint pair, optionally naming a rank:

```sql
FETCH PROP ON knows 1->2;         // every rank of this pair
FETCH PROP ON knows 1->2@7;       // only rank 7
FETCH PROP ON knows 1->2@7, 3->4; // each reference carries its own rank
FETCH PROP ON * 1->2;
FETCH PROP ON knows "alice"->"bob";
```

An **omitted rank matches every rank** of the pair, which is why the result
includes a `ranking` field. This differs from `INSERT EDGE`, `DELETE EDGE`, and
`UPDATE EDGE`, where an omitted rank means rank `0` — those statements address
one edge, while a fetch without a rank is a query over the pair. Name the rank
when you want exactly one edge.

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

### Matching more than one node label

A vertex carries a **set** of tags, so the separator says how several labels
combine:

```sql
MATCH (a:animal:pet) RETURN id(a) AS vid;   // every listed tag
MATCH (a:animal|pet) RETURN id(a) AS vid;   // at least one of them
```

`:` is an intersection and `|` is a union. Both are order-independent, and an
intersection nothing satisfies is empty. Naming no label matches every vertex.

Mixing the two separators in one pattern (`(a:A:B|C)`) has no obvious precedence
and is refused.

This is deliberately **not** symmetric with edge patterns below, because the data
model is not: an edge has exactly one type, so `[e:a|b]` can only mean union,
while a vertex can hold several tags at once.

### Matching more than one edge type

Separate types with `|`. The pattern matches an edge of **any** listed type — it
is a union, not an intersection:

```sql
MATCH (a:person)-[e:knows|follows]->(b:person) RETURN id(b) AS vid;
MATCH (a:person)-[:knows|follows|blocks*1..2]->(b:person) RETURN id(b) AS vid;
```

Naming no type at all matches every edge type:

```sql
MATCH (a:person)-[e]->(b) RETURN type(e) AS kind;
```

A repeated colon (`[e:knows:follows]`) means the same thing and still parses,
because it was the only spelling that worked before `|` was accepted. Prefer `|`:
it is what nGQL and Cypher use, so those queries port unchanged.

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

**Current MATCH contract:**
projection, WHERE, grouping and aggregate evaluation propagate storage, decode
and VID-resolution failures rather than converting them to missing values.
Genuinely missing properties and typed nulls retain their meanings; a directly
addressed VID that does not exist remains no match. Decoded MATCH candidates,
directional/optional edges and intermediate vertices are identity-checked.
Required/OPTIONAL bound starts recheck their labels and literal properties.
Undirected self-loops count once by full edge identity; distinct ranks and opposite
edges remain distinct. Required/OPTIONAL bound destinations cannot be overwritten:
repeated node variables keep the same VID and repeated edge variables keep full
edge identity. A node/edge binding-kind collision raises an error when encountered.
Newly introduced OPTIONAL null bindings cannot later be rebound. Validation
concerns decoded records, not an exhaustive integrity scan.

`COUNT(expr)` excludes nulls and `MIN/MAX` skip them. A global aggregate over
empty input produces one row before `LIMIT/OFFSET`; result windows apply after
aggregation and do not bypass aggregate overflow checks. Integer-only `SUM`
returns an exact integer or an overflow error. Floating `SUM/AVG` reject
nonfinite results; this does not define a total ordering for mixed-type or
nonfinite `MIN/MAX` inputs.

Grouping uses canonical typed keys: map entry order is ignored, vertices/edges
use their graph identity, and floating `+0`/`-0` group together. Integers and
floats remain different groups. Nonfinite values and Path/Set/DataSet/Geography
group keys are explicitly unsupported. `ORDER BY` must name an unambiguous output
alias or exactly match a projected expression; unknown/ambiguous references fail
before scanning. Aggregate output rows are checked against the shared estimated
result-memory budget; this does not cap all intermediate grouping memory.

The planner rejects unsupported MATCH arithmetic, unary and map-constructor
expressions; `COLLECT`; nested/misplaced aggregates; and nonliteral node/edge
pattern properties before reading data, even for empty input. Aggregates are
supported only as top-level RETURN calls; they cannot be nested inside scalar
functions/lists or used as WHERE/GROUP BY expressions. This restriction does
not prohibit property maps returned by `properties()`. The current source
also accepts `isa` as an alias for `is_a`. The reverse lookup shortcut is limited
to outgoing patterns without a range, and `EXPLAIN` applies the same direction/range
restriction. This slice is not a claim of complete MATCH semantics; QRY-2 remains
IN PROGRESS.

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

Supported MATCH aggregates are `COUNT`, `SUM`, `AVG`, `MIN` and `MAX`.
`COLLECT` is not implemented; the current source rejects it before data access.
`COUNT(*)` counts rows, while `COUNT(expr)` excludes null values.
The eligible plain single-node `COUNT(*)` path streams validated candidates;
grouped and edge counts use normal matching. Both retain their applicable scan
budgets and result-window semantics.

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

`LOOKUP` targets a tag or an edge type. The bare form resolves the name against
the schema:

```sql
LOOKUP ON person WHERE person.name == "Alice";
LOOKUP ON person WHERE person.age >= 21 YIELD person.name, person.age LIMIT 20;

LOOKUP ON knows WHERE knows.since == 2020;
LOOKUP ON knows WHERE since == 2020 LIMIT 20;
```

A space may hold a tag and an edge type of the **same name**, and the bare form
resolves such a name to the **tag**. Use `LOOKUP ON EDGE` to address the edge,
which is otherwise unreachable — a predicate over its properties would match the
tag and report nothing:

```sql
CREATE TAG dup(x INT64);
CREATE EDGE dup(y INT64);

LOOKUP ON dup WHERE dup.x == 7;        // the tag
LOOKUP ON EDGE dup WHERE dup.y == 9;   // the edge
```

There is no `LOOKUP ON TAG`: the bare form already prefers a tag, and an explicit
spelling that still fell back to an edge when no such tag existed would be
misleading.

A tag `LOOKUP` returns `<tag>.vid` and the tag's properties. An edge `LOOKUP`
returns **`<edge>.src`, `<edge>.dst`, and `<edge>.rank`** — the three together
identify one edge, since rank is part of an edge's identity. Properties are
matched bare or qualified by name (`since` and `knows.since` both resolve),
as they are for tags.

An equality predicate on an indexed property can use the secondary index, for
both tags and edge types. Eligible ordered ranges use single-field indexes in
standalone mode; other supported predicates use a bounded predicate scan:

- Ordered range predicates (`>`, `>=`, `<`, `<=`) on an **edge type**
  use the index only with a compatible property schema and boundary.
  Float/Double properties use predicate scans to cover both integer and float key domains.
  `EXPLAIN` uses the same choice. String boundaries fall back to a scan because
  their length-prefixed index encoding is not lexical.
- Ordered range predicates on a **tag** use a single-field index when a Boolean
  or integer literal matches the schema type. Float/Double properties fall back
  to a scan because integer and float values occupy different index key domains.
- Ordered `LOOKUP` ranges are rejected in distributed execution.
- A predicate the pushdown cannot express (`CONTAINS`, `STARTS WITH`, or a
  field-to-field comparison) is **rejected** rather than silently matching
  everything.

The default scan budget is 100,000 candidates, configured by
`BYORIDB_MAX_SCAN_LIMIT`.

**Current source contract:** local tag/edge LOOKUP
counts raw candidates in each full-prefix or index-partition scan, before
predicate filtering and stale-entry revalidation. `0` disables that budget.
Exactly the cap may succeed; observing candidate cap+1 fails the query instead
of returning a truncated success. Query `LIMIT/OFFSET` controls the result window
independently: even `LIMIT 1` cannot hide a candidate-budget overflow. This is a
per-scan budget, not a global total-work, edge-count or memory bound. Relevant
index-partition, schema and decode failures propagate, and malformed edge-index
keys fail. LOOKUP and FIND share an edge decoder that rejects malformed JSON,
invalid structural member types and explicit key/payload identity mismatches;
LOOKUP also validates vertex records. Legacy omitted vertex VID, edge rank and
properties retain their defaults. Validation covers records actually decoded:
LIMIT may stop later decoding and FIND can skip unrelated edge types. It does not
validate every acquired candidate, detect all corruption or validate FIND vertices.
Distributed indexed LOOKUP validates the complete partition topology, applies
this raw budget independently to every partition and follows query-bound keyset
cursors. It fetches payloads from the owning storage service, validates their
identity and typed properties, and fails on a missing or failed partition.
Result limits do not split the budget between partitions. Pages do not establish
a snapshot across concurrent writes; distributed full scans and ordered ranges
remain unsupported. These component guarantees do not enable the standalone
launcher to run a replicated cluster.

Float equality uses the existing epsilon comparison, so indexed execution scans
and rechecks the entire selected index instead of fetching one exact float key.
Every candidate counts toward the scan budget. Integer and float equality remain
type-strict; an integer literal does not match a stored float merely because
both have the same numeric magnitude.

Index candidates are always revalidated against the stored vertex or edge, so a
stale index entry cannot become a result. Use `EXPLAIN` or `PROFILE` to see
which access path ran:

```
EXPLAIN LOOKUP ON knows WHERE knows.since == 2020;
// IndexScan  on Edge knows where knows.since == 2020   index: knows_since_idx

EXPLAIN LOOKUP ON knows WHERE knows.since > 2019;
// IndexScan  on Edge knows where knows.since > 2019    index: knows_since_idx

EXPLAIN LOOKUP ON tagged WHERE tagged.label > 'a';
// EdgeScan   on Edge tagged where tagged.label > "a"   ⚠ FULL SCAN
```

## FIND paths

```sql
FIND SHORTEST PATH FROM 1 TO 3 OVER knows;
FIND SHORTEST PATH FROM 1 TO 3 OVER road WEIGHT BY distance;
FIND SHORTEST PATH FROM 1 TO 3 OVER knows BIDIRECT UPTO 5 STEPS;
FIND ALL SHORTEST PATHS FROM 1 TO 3 OVER knows UPTO 5 STEPS;
```

The `OVER` target is one edge type or `*`, not a comma-separated edge list.
`FIND WHERE` and `FIND YIELD` are not implemented and are rejected explicitly
before resolving vertex IDs.

**Current source contract:**

- `max_traversal_nodes` charges expanded vertices, or vertex/hop states for
  hop-bounded weighted queries. Its default is 100,000; `0` disables it. Exceeding
  it fails the query. It does not bound queued frontier memory or scanned edges.
- `FIND ALL SHORTEST PATHS` uses `max_find_paths` (default 1,024; `0` disables).
  Exactly that many complete paths may succeed; finding another complete path
  fails rather than truncating the output.
- Explicit `UPTO n STEPS` limits weighted as well as unweighted paths. Without
  `UPTO`, unweighted queries default to 10 steps while weighted queries have no
  hop bound; the execution work budget still applies. `UPTO` values above
  `u32::MAX` are rejected during parsing rather than wrapped.
- A missing or nonnumeric `WEIGHT BY` property retains the legacy unit-weight
  default of `1`. Numeric weights must be nonnegative and finite. Conservative
  validation of reachable weights can do work beyond the returned path;
  negative/nonfinite numeric weights or relevant corrupt edge data cause an
  error. Finding a cheap target path early does not finish validation/execution.

These contracts describe the current checkout; older releases are unchanged.
See [runtime guards](../../reference/performance.md#runtime-guards) for limits
and the remaining execution boundaries.

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

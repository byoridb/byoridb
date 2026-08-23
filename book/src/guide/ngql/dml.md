[한국어](../../ko/guide/ngql/dml.html)

# Data mutation

Select a space and create the referenced tag or edge schema before mutating
graph data. Use integer literals in `INT64` spaces and quoted string literals in
`FIXED_STRING(N)` spaces.

## Insert vertices

```sql
INSERT VERTEX person(name, age) VALUES 1:("Alice", 30);
```

For a string-VID space, the same mutation uses quoted endpoints:

```sql
INSERT VERTEX person(name, age) VALUES "alice":("Alice", 30);
INSERT EDGE knows(since) VALUES "alice"->"bob":(2020);
```

A statement can batch several vertices into one current-view/history
transaction:

```sql
INSERT VERTEX person(name, age) VALUES
    1:("Alice", 30),
    2:("Bob", 25),
    3:("Carol", 28);
```

If a VID already exists, INSERT replaces that vertex's current tag set. If the
same VID appears more than once in one statement, the last row is the current
view. Tag-to-VID entries removed by the overwrite, including tags from earlier
duplicate rows, are deleted in the same graph-data transaction; label-only
MATCH and COUNT therefore do not retain those old labels.

Only named properties are supplied. Unknown property names and plainly
incompatible scalar types are rejected. The current INSERT path does not fill
omitted properties from schema defaults, so pass required values explicitly or
enforce them with a shape.

Numeric list literals are accepted and can be used for recommendation
embeddings:

```sql
CREATE TAG product(name STRING, embedding STRING NULL);
INSERT VERTEX product(name, embedding) VALUES
    1001:("Widget", [0.12, -0.04, 0.88]);
```

There is no list property type in the current DDL parser. Composite values are
validated leniently once the property name exists, so the example uses a
placeholder scalar declaration for `embedding`.

## Insert edges

The default ranking is `0`:

```sql
INSERT EDGE knows(since) VALUES 1->2:(2020), 2->3:(2021);
INSERT EDGE empty_relation() VALUES 1->3:();
```

Use `@<rank>` to store parallel edges with the same type and endpoints:

```sql
INSERT EDGE knows(since) VALUES 1->2@1:(2020), 1->2@2:(2024);
```

Semantic edge declarations may cause additional inferred edges or class
memberships to be materialized after an asserted edge is inserted.

## Update vertices

```sql
UPDATE VERTEX ON person 1 SET age = 31;
UPDATE VERTEX ON person 1 SET name = "Alicia", age = 32;
```

`WHEN` is evaluated against the current vertex and makes the statement a no-op
when false:

```sql
UPDATE VERTEX ON person 1 SET age = 33 WHEN age == 32;
```

Current assignment planning accepts literal and list values. Arithmetic
assignments such as `SET score = score + 1` are not operational.

Without `WHEN`, `UPDATE VERTEX` behaves as an upsert and creates the vertex/tag
when the VID does not exist. If your application requires update-only behavior,
check existence first or use a true `WHEN` predicate tied to current data.

## Update edges

```sql
UPDATE EDGE ON knows 1->2 SET since = 2021;
UPDATE EDGE ON knows 1->2@7 SET since = 1991;
UPDATE EDGE ON knows 1->2 SET since = 2022 WHEN knows.since == 2021;
```

The rank defaults to `0` when omitted, and it is part of the edge's identity, so
`1->2` and `1->2@7` are different edges and an update touches only the one it
names. Source, destination, type, and rank cannot be assigned — an update changes
properties, never which edge it is.

`WHEN` is evaluated against the edge's current properties, which are visible
both bare and qualified by edge type (`since` and `knows.since`).

**Unlike `UPDATE VERTEX`, this is not an upsert.** Updating an edge that does not
exist returns `0` and creates nothing, because creating one here would have to
maintain the degree counters and asserted ontology triples that `INSERT EDGE`
does. Use `INSERT EDGE` to create.

Both directions see the change: the reverse-traversal copy of the edge is
rewritten in the same transaction as the forward one, and edge indexes are moved
off the old property values onto the new ones.

## Delete vertices

```sql
DELETE VERTEX 1;
DELETE VERTEX 1, 2, 3;
DELETE VERTEX 7 WHERE status == "inactive";
```

Deleting a vertex removes its vertex record, tag-index entries, embeddings, and
applicable inferred materialization. It does **not** currently guarantee a
cascade over all asserted incident edges. Delete those edges explicitly when
referential cleanup matters.

Vertices involved in an irreversible `sameAs` merge cannot be deleted.

## Delete edges

```sql
DELETE EDGE knows 1->2;
DELETE EDGE knows 1->2@1, 2->3@2;
```

Deleting an asserted semantic edge retracts inferred consequences through the
current inference-maintenance path. A `sameAs` edge cannot be deleted because
unmerging canonical vertices is not implemented.

## Atomicity and history

- A multi-row vertex or edge INSERT applies its current-view records and
  history versions in one storage transaction. Vertex overwrites include their
  tag-to-VID additions and removals in that transaction.
- For `FIXED_STRING`, every deterministic schema, shape, VID-length, endpoint,
  and `WHEN` validation completes before a new mapping is claimed. Mapping
  uniqueness uses a separate atomic reverse-key claim before the graph-data
  transaction. Therefore a storage failure after that claim can leave an
  unused mapping record even though no graph row committed; mappings are not
  recycled. The all-or-nothing statement guarantee applies to graph current
  view, tag-to-VID state, and history, not to cleanup of such unused mapping
  metadata after an I/O failure.
- Current UPDATE and DELETE paths also write the current view and temporal
  history together.
- Separate nGQL requests are not part of a cross-statement transaction.
- Semicolon-separated compound statements are sequential, not transactional:
  if a later clause fails, successful earlier clauses are not rolled back.
- Each version receives the server's epoch-millisecond transaction time. The
  one `AS OF` timestamp is applied to both current valid-time and transaction-
  time axes in the current temporal interface.

See [Data queries](./dql.md) for vertex and edge `FETCH PROP ... AS OF` reads.

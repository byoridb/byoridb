[한국어](../../ko/guide/ngql/dml.html)

# Data mutation

Select a space and create the referenced tag or edge schema before mutating
graph data. Current execution requires integer vertex IDs.

## Insert vertices

```sql
INSERT VERTEX person(name, age) VALUES 1:("Alice", 30);
```

A statement can batch several vertices into one current-view/history
transaction:

```sql
INSERT VERTEX person(name, age) VALUES
    1:("Alice", 30),
    2:("Bob", 25),
    3:("Carol", 28);
```

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

`UPDATE EDGE` is parsed but its execution plan currently discards the edge
identity and fails. Replace an edge by deleting and reinserting it until a
dedicated edge-update executor is implemented.

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
  history versions in one storage transaction.
- Current UPDATE and DELETE paths also write the current view and temporal
  history together.
- Separate nGQL requests are not part of a cross-statement transaction.
- Semicolon-separated compound statements are sequential, not transactional:
  if a later clause fails, successful earlier clauses are not rolled back.
- Each version receives the server's epoch-millisecond transaction time. The
  one `AS OF` timestamp is applied to both current valid-time and transaction-
  time axes in the current temporal interface.

See [Data queries](./dql.md) for vertex and edge `FETCH PROP ... AS OF` reads.

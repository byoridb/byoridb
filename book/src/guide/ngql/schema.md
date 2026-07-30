[한국어](../../ko/guide/ngql/schema.html)

# Schema

Select a space before running the schema statements on this page.

```sql
USE social;
```

## Tags

A tag describes a vertex type and its properties.

```sql
CREATE TAG person(
    name STRING DEFAULT "unknown",
    age INT64 NULL,
    active BOOL DEFAULT true
);
```

`IF NOT EXISTS` is supported:

```sql
CREATE TAG IF NOT EXISTS person(name STRING, age INT64);
```

Supported property declarations are `BOOL`, `INT8`, `INT16`, `INT32`, `INT64`
(`INT` is an alias), `FLOAT`, `DOUBLE`, `STRING`, `TIMESTAMP`, `DATE`, `TIME`,
and `DATETIME`. In `CREATE TAG`/`CREATE EDGE`, append `NULL` to mark a property
nullable; omission records it as non-nullable. `NOT NULL` spelling is supported
by `ALTER ... ADD/CHANGE`, not by the create-property parser.

Defaults and nullability are stored in schema metadata, but the current INSERT
path does not synthesize omitted default values or enforce every missing
non-null field. Supply required values explicitly or enforce them with a shape.

Inspect or remove tags with:

```sql
SHOW TAGS;
DESCRIBE TAG person;
SHOW CREATE TAG person;
DROP TAG IF EXISTS person;
```

`DESC TAG person` is an alias for `DESCRIBE TAG person`.

## Alter a tag

The parser and local executor support adding, dropping, and changing schema
columns:

```sql
ALTER TAG person ADD (email STRING NULL);
ALTER TAG person DROP (email);
ALTER TAG person CHANGE (age INT32 NULL);
```

An `ADD` column is nullable by default. For a new non-nullable column, provide a
default:

```sql
ALTER TAG person ADD (verified BOOL NOT NULL DEFAULT false);
```

Schema changes update schema metadata; plan migrations and verify existing rows
before relying on a changed type.

## Edge types

```sql
CREATE EDGE knows(since INT64, strength DOUBLE DEFAULT 1.0);
CREATE EDGE IF NOT EXISTS follows(since TIMESTAMP);
```

Edge schema inspection and changes mirror tag syntax:

```sql
SHOW EDGES;
DESCRIBE EDGE knows;
SHOW CREATE EDGE knows;
ALTER EDGE knows ADD (source STRING NULL);
DROP EDGE IF EXISTS knows;
```

## Semantic edge declarations

ByoriDB can materialize ontology entailments into the current graph view. The
following clauses may follow an edge property list:

| Clause | Effect |
| --- | --- |
| `TRANSITIVE` | Derive `a -> c` from `a -> b` and `b -> c` |
| `SYMMETRIC` | Derive the reverse edge |
| `INVERSE OF other` | Materialize the corresponding reverse `other` edge |
| `SUBPROPERTY OF other` | Materialize the same endpoints under `other` |
| `EQUIVALENT TO other` | Treat the two properties as equivalent for materialization |
| `CHAIN first, second` | Derive the declared edge from the property chain |
| `DOMAIN class` | Infer the source vertex's class |
| `RANGE class` | Infer the destination vertex's class |

Referenced edge types and classes must already exist. For example:

```sql
CREATE CLASS city(name STRING);
CREATE EDGE related();
CREATE EDGE parent();
CREATE EDGE ancestor() TRANSITIVE SUBPROPERTY OF related;
CREATE EDGE child() INVERSE OF parent;
CREATE EDGE born_in() RANGE city;
```

Insertions extend the materialized closure. Current `DELETE EDGE` performs
incremental inference retraction, while `DELETE VERTEX` rematerializes the
space. Historical `AS OF` reads are designed around asserted DML versions;
do not rely on them to reproduce every past inferred fact.

Explain an inferred edge's recorded derivation with:

```sql
WHY 1 -> 3 OVER ancestor;
```

The reserved `sameAs` edge name performs an irreversible canonical-vertex merge
when such an edge is inserted. Deleting the `sameAs` edge or an involved vertex
is rejected because unmerge is not implemented.

## Classes and consistency

A class also creates a compatible tag schema and can declare hierarchy,
equivalence, and disjointness:

```sql
CREATE CLASS animal(name STRING);
CREATE CLASS pet(owner STRING);
CREATE CLASS dog(breed STRING) SUBCLASS OF animal, pet;
CREATE CLASS person_kind(name STRING);
CREATE CLASS human(name STRING) EQUIVALENT TO person_kind;
CREATE CLASS building() DISJOINT WITH animal;
```

Referenced superclasses and equivalent classes must already exist. Inspect and
validate the model with:

```sql
SHOW CLASSES;
DESCRIBE CLASS dog;
CHECK CONSISTENCY;
DROP CLASS IF EXISTS dog;
```

Use `is_a(vertex, "class")` in a `MATCH` predicate for hierarchy-aware class
membership.

## Shapes

Shapes add required-property, datatype, and value-predicate checks to a target
class:

```sql
CREATE CLASS profile(email STRING, age INT);
CREATE SHAPE profile_shape ON profile (
    email STRING REQUIRED,
    age INT,
    age CHECK age >= 0
);

CHECK SHAPE;
DROP SHAPE IF EXISTS profile_shape;
```

Writes are validated against applicable shapes, and `CHECK SHAPE` reports
violations already present in the graph.

## Indexes

Create and inspect secondary indexes in the selected space:

```sql
CREATE TAG INDEX person_name_idx ON person(name);
CREATE EDGE INDEX knows_since_idx ON knows(since);

SHOW TAG INDEXES;
SHOW EDGE INDEXES;
DESCRIBE TAG INDEX person_name_idx;
DESCRIBE EDGE INDEX knows_since_idx;
```

The current drop syntax places `INDEX` before the schema kind:

```sql
DROP INDEX TAG IF EXISTS person_name_idx;
DROP INDEX EDGE IF EXISTS knows_since_idx;
```

Tag indexes support `LOOKUP` acceleration and are maintained across current
INSERT, UPDATE, and DELETE paths. Edge-type `LOOKUP` is not currently wired,
even though edge index DDL exists.

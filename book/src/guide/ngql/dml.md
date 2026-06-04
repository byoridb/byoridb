# Data Manipulation

Insert, update, and delete vertices and edges.

## Vertices

### INSERT VERTEX

```sql
INSERT VERTEX <tag_name>(<prop1>, <prop2>, ...)
VALUES <vid>:(<value1>, <value2>, ...);
```

**Single vertex:**

```sql
INSERT VERTEX person(name, age) VALUES 1:('Alice', 30);
INSERT VERTEX player(name, score) VALUES 100:('Bob', 2500);
```

**Multiple vertices:**

```sql
INSERT VERTEX person(name, age) VALUES
    1:('Alice', 30),
    2:('Bob', 25),
    3:('Carol', 28);
```

**With default values:**

```sql
-- If player has (name STRING, score INT64 DEFAULT 0)
INSERT VERTEX player(name) VALUES 1:('NewPlayer');
-- score will be 0
```

### UPDATE VERTEX

```sql
UPDATE VERTEX ON <tag_name> <vid>
SET <property> = <value>, ...
[WHEN <condition>];
```

**Examples:**

```sql
UPDATE VERTEX ON person 1 SET age = 31;
UPDATE VERTEX ON player 100 SET score = score + 100;
UPDATE VERTEX ON person 1 SET name = 'Alicia', age = 32;
```

**Conditional update:**

```sql
UPDATE VERTEX ON player 100
SET score = score + 500
WHEN score > 1000;
```

### DELETE VERTEX

```sql
DELETE VERTEX <vid> [, <vid>, ...];
```

**Examples:**

```sql
DELETE VERTEX 1;
DELETE VERTEX 1, 2, 3;
```

> **Note:** Deleting a vertex also deletes all edges connected to it.

## Edges

### INSERT EDGE

```sql
INSERT EDGE <edge_name>(<prop1>, <prop2>, ...)
VALUES <src_vid>-><dst_vid>:(<value1>, <value2>, ...);
```

**Single edge:**

```sql
INSERT EDGE knows() VALUES 1->2:();
INSERT EDGE follow(since) VALUES 1->2:(1609459200);
INSERT EDGE purchase(quantity, price) VALUES 100->200:(2, 29.99);
```

**Multiple edges:**

```sql
INSERT EDGE follow(since) VALUES
    1->2:(1609459200),
    1->3:(1612137600),
    2->3:(1614556800);
```

**With ranking (for multiple edges between same vertices):**

```sql
INSERT EDGE follow(since) VALUES 1->2@1:(1609459200);
INSERT EDGE follow(since) VALUES 1->2@2:(1612137600);
```

### UPDATE EDGE

```sql
UPDATE EDGE ON <edge_name> <src_vid>-><dst_vid>[@<rank>]
SET <property> = <value>, ...;
```

**Examples:**

```sql
UPDATE EDGE ON follow 1->2 SET since = 1609459200;
UPDATE EDGE ON purchase 100->200 SET quantity = 3, price = 25.99;
```

### DELETE EDGE

```sql
DELETE EDGE <edge_name> <src_vid>-><dst_vid>[@<rank>];
```

**Examples:**

```sql
DELETE EDGE knows 1->2;
DELETE EDGE follow 1->2@1;
```

## Batch Operations

For bulk data loading, use batch inserts:

```sql
INSERT VERTEX person(name, age) VALUES
    1:('User1', 20),
    2:('User2', 21),
    3:('User3', 22),
    -- ... up to 1000 vertices per batch
    1000:('User1000', 30);
```

## Transaction Notes

- Each statement executes atomically
- Batch inserts in a single statement are atomic
- Cross-statement transactions are not yet supported

# Data Query

Query and traverse your graph data.

## FETCH PROP

Retrieve properties of specific vertices:

```sql
FETCH PROP ON <tag_name> <vid> [, <vid>, ...];
```

**Examples:**

```sql
-- Single vertex
FETCH PROP ON person 1;

-- Multiple vertices
FETCH PROP ON person 1, 2, 3;

-- All tags on a vertex
FETCH PROP ON * 1;
```

## GO (Graph Traversal)

Traverse the graph following edges:

```sql
GO FROM <vid> [, <vid>, ...]
OVER <edge_name> [, <edge_name>, ...]
[REVERSELY]
[YIELD <expression> [AS <alias>], ...];
```

**Basic traversal:**

```sql
-- Find who user 1 follows
GO FROM 1 OVER follow;

-- Traverse multiple edges
GO FROM 1 OVER follow, knows;

-- Reverse traversal (find followers)
GO FROM 1 OVER follow REVERSELY;
```

**Multi-hop traversal:**

```sql
-- 2-hop traversal
GO 2 STEPS FROM 1 OVER follow;

-- 1 to 3 hops
GO 1 TO 3 STEPS FROM 1 OVER follow;
```

**With YIELD:**

```sql
GO FROM 1 OVER follow
YIELD $$.person.name AS friend_name, $$.person.age AS friend_age;

GO FROM 1 OVER purchase
YIELD properties(edge).quantity AS qty, properties(edge).price AS price;
```

**Special variables:**

| Variable | Description |
|----------|-------------|
| `$$` | Destination vertex |
| `$^` | Source vertex |
| `$-` | Input from pipe |

## MATCH (Pattern Matching)

Cypher-style pattern matching:

```sql
MATCH <pattern>
[WHERE <condition>]
RETURN <expression> [AS <alias>], ...
[ORDER BY <expression> [ASC|DESC]]
[LIMIT <n>];
```

**Find vertices:**

```sql
-- All persons
MATCH (n:person) RETURN n;

-- With filter
MATCH (n:person) WHERE n.age > 25 RETURN n.name, n.age;

-- With limit
MATCH (n:person) RETURN n LIMIT 10;
```

**Find paths:**

```sql
-- One-hop
MATCH (a:person)-[e:follow]->(b:person)
RETURN a.name, b.name;

-- With conditions
MATCH (a:person)-[e:follow]->(b:person)
WHERE a.name = 'Alice' AND b.age > 20
RETURN b.name, b.age;

-- Variable-length paths
MATCH (a:person)-[e:follow*1..3]->(b:person)
WHERE a.name = 'Alice'
RETURN b.name;
```

## LOOKUP (Index Query)

Query vertices or edges using indexes:

```sql
LOOKUP ON <tag_name|edge_name>
[WHERE <condition>]
[YIELD <expression>, ...];
```

**Examples:**

```sql
-- Find by indexed property
LOOKUP ON person WHERE person.name == 'Alice';

-- With yield
LOOKUP ON person
WHERE person.age > 25
YIELD person.name, person.age;

-- Edge lookup
LOOKUP ON follow
WHERE follow.since > 1609459200
YIELD src(edge), dst(edge);
```

> **Note:** LOOKUP requires an index on the queried property.

## FIND PATH

Find paths between vertices:

```sql
FIND SHORTEST PATH FROM <src_vid> TO <dst_vid> OVER <edge_name>;
FIND SHORTEST PATH FROM <src_vid> TO <dst_vid> OVER <edge_name> WEIGHT BY <property>;
FIND ALL PATH FROM <src_vid> TO <dst_vid> OVER <edge_name>;
```

**Examples:**

```sql
-- Shortest path
FIND SHORTEST PATH FROM 1 TO 100 OVER follow;

-- Weighted shortest path
FIND SHORTEST PATH FROM 1 TO 100 OVER road WEIGHT BY distance;

-- All paths (with limit)
FIND ALL PATH FROM 1 TO 100 OVER follow UPTO 5 STEPS;

-- With multiple edges
FIND SHORTEST PATH FROM 1 TO 100 OVER follow, knows;
```

## Aggregations

Use with YIELD or RETURN:

```sql
-- Count
MATCH (n:person) RETURN count(n);

-- Sum, Avg, Min, Max
GO FROM 1 OVER purchase YIELD sum(properties(edge).price);

MATCH (n:person)
WHERE n.age > 20
RETURN avg(n.age), max(n.age), min(n.age);
```

## Combining Queries (Pipe)

Chain queries with pipe operator:

```sql
GO FROM 1 OVER follow YIELD dst(edge) AS id
| GO FROM $-.id OVER follow YIELD dst(edge);
```

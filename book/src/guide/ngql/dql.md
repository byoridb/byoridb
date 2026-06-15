# 데이터 쿼리

그래프 데이터를 쿼리하고 순회합니다.

## FETCH PROP

특정 버텍스의 속성을 조회합니다:

```sql
FETCH PROP ON <tag_name> <vid> [, <vid>, ...];
```

**예시:**

```sql
-- Single vertex
FETCH PROP ON person 1;

-- Multiple vertices
FETCH PROP ON person 1, 2, 3;

-- All tags on a vertex
FETCH PROP ON * 1;
```

## GO (그래프 순회)

엣지를 따라 그래프를 순회합니다:

```sql
GO FROM <vid> [, <vid>, ...]
OVER <edge_name> [, <edge_name>, ...]
[REVERSELY]
[YIELD <expression> [AS <alias>], ...];
```

**기본 순회:**

```sql
-- Find who user 1 follows
GO FROM 1 OVER follow;

-- Traverse multiple edges
GO FROM 1 OVER follow, knows;

-- Reverse traversal (find followers)
GO FROM 1 OVER follow REVERSELY;
```

**멀티홉 순회:**

```sql
-- 2-hop traversal
GO 2 STEPS FROM 1 OVER follow;

-- 1 to 3 hops
GO 1 TO 3 STEPS FROM 1 OVER follow;
```

**YIELD 사용:**

```sql
GO FROM 1 OVER follow
YIELD $$.person.name AS friend_name, $$.person.age AS friend_age;

GO FROM 1 OVER purchase
YIELD properties(edge).quantity AS qty, properties(edge).price AS price;
```

**특수 변수:**

| 변수 | 설명 |
|----------|-------------|
| `$$` | 대상 버텍스 |
| `$^` | 출발 버텍스 |
| `$-` | 파이프로부터의 입력 |

## MATCH (패턴 매칭)

Cypher 스타일의 패턴 매칭:

```sql
MATCH <pattern>
[WHERE <condition>]
RETURN <expression> [AS <alias>], ...
[ORDER BY <expression> [ASC|DESC]]
[LIMIT <n>];
```

**버텍스 찾기:**

```sql
-- All persons
MATCH (n:person) RETURN n;

-- With filter
MATCH (n:person) WHERE n.age > 25 RETURN n.name, n.age;

-- With limit
MATCH (n:person) RETURN n LIMIT 10;
```

**경로 찾기:**

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

## LOOKUP (인덱스 쿼리)

인덱스를 사용하여 버텍스나 엣지를 쿼리합니다:

```sql
LOOKUP ON <tag_name|edge_name>
[WHERE <condition>]
[YIELD <expression>, ...];
```

**예시:**

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

> **참고:** LOOKUP은 쿼리 대상 속성에 인덱스가 필요합니다.

## FIND PATH

버텍스 사이의 경로를 찾습니다:

```sql
FIND SHORTEST PATH FROM <src_vid> TO <dst_vid> OVER <edge_name>;
FIND SHORTEST PATH FROM <src_vid> TO <dst_vid> OVER <edge_name> WEIGHT BY <property>;
FIND ALL PATH FROM <src_vid> TO <dst_vid> OVER <edge_name>;
```

**예시:**

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

## 집계

YIELD 또는 RETURN과 함께 사용합니다:

```sql
-- Count
MATCH (n:person) RETURN count(n);

-- Sum, Avg, Min, Max
GO FROM 1 OVER purchase YIELD sum(properties(edge).price);

MATCH (n:person)
WHERE n.age > 20
RETURN avg(n.age), max(n.age), min(n.age);
```

## 쿼리 결합 (파이프)

파이프 연산자로 쿼리를 연결합니다:

```sql
GO FROM 1 OVER follow YIELD dst(edge) AS id
| GO FROM $-.id OVER follow YIELD dst(edge);
```

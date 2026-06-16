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

## RECOMMEND (유사 버텍스 추천)

특정 버텍스와 가장 유사한 버텍스 top-k를 추천합니다. 유사도 정의는 두 가지입니다.

```sql
RECOMMEND SIMILAR TO <vid> OVER <edge>[, <edge> ...]|* [WHERE <조건>] [LIMIT k];
RECOMMEND SIMILAR TO <vid> BY EMBEDDING <prop> [WHERE <조건>] [LIMIT k];
```

기본 `LIMIT`은 10입니다.

### 구조적 유사도 (OVER)

공유 이웃 겹침(Jaccard)으로 계산합니다: `sim(a,b) = |N(a)∩N(b)| / |N(a)∪N(b)|`.
`N(v)`는 지정한 edge 타입에 대한 `v`의 out-이웃 집합이며, `OVER *`는 모든 edge
타입을 뜻합니다. 상품을 공유 속성 노드(브랜드·카테고리·스펙)로 연결해두면 "공유
이웃이 많을수록 유사"로 동작합니다. 결과 컬럼은 `vid / score / shared`.

```sql
-- has_brand, in_category로 연결된 공유 속성 기준 유사 상품 5개
RECOMMEND SIMILAR TO 1001 OVER has_brand, in_category LIMIT 5;
```

### 임베딩 유사도 (BY EMBEDDING)

리스트형 임베딩 속성에 대한 **코사인 최근접 이웃**입니다. 채널마다 제목 표기가
달라도 의미가 가까우면 매칭됩니다. 임베딩 벡터는 외부 모델이 생성해 INSERT 시
숫자 리스트 속성으로 넣습니다(DB는 저장·검색만 담당). 결과 컬럼은 `vid / score`.

```sql
-- 임베딩 속성을 가진 버텍스 삽입 (벡터는 외부에서 계산)
INSERT VERTEX product(emb) VALUES 1001:([0.12, -0.04, 0.88, ...]);

-- 1001과 의미상 가장 가까운 상품
RECOMMEND SIMILAR TO 1001 BY EMBEDDING emb LIMIT 5;
```

### WHERE 필터 (하이브리드)

후보를 속성 술어로 필터링합니다. `seed.<prop>`은 시드(기준) 버텍스의 속성을
가리키므로 "시드와 다른 채널" 같은 상대 비교를 값 하드코딩 없이 표현할 수 있습니다.

```sql
-- 1001과 유사하되 'coupang' 채널인 상품
RECOMMEND SIMILAR TO 1001 BY EMBEDDING emb WHERE channel = "coupang" LIMIT 5;

-- 1001과 유사하되 시드와 '다른' 채널인 상품
RECOMMEND SIMILAR TO 1001 BY EMBEDDING emb WHERE channel != seed.channel LIMIT 5;
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

[English](../../../guide/ngql/dql.html)

# 데이터 조회

조회 문은 인증 세션이 선택한 스페이스를 사용합니다.

## FETCH PROP

정수 VID로 버텍스를 가져옵니다.

```sql
FETCH PROP ON person 1;
FETCH PROP ON person 1, 2, 3;
FETCH PROP ON * 1;
```

Endpoint 쌍으로 edge를 가져옵니다.

```sql
FETCH PROP ON knows 1->2;
FETCH PROP ON * 1->2;
```

현재 temporal 읽기 표면은 vertex와 edge에 epoch millisecond 시각을 받습니다.

```sql
FETCH PROP ON person 1 AS OF 1785283200000;
FETCH PROP ON knows 1->2 AS OF 1785283200000;
```

`AS OF`는 같은 시각을 valid time과 transaction time에 적용해 asserted history를
해석합니다. 과거 MATCH, GO, RECOMMEND는 구현되지 않았고 과거 inferred fact의
재구성도 보장되지 않습니다.

## GO

하나 이상의 outgoing edge 타입을 따라갑니다.

```sql
GO FROM 1 OVER knows;
GO FROM 1 OVER knows, follows;
GO FROM 1 OVER *;
```

정확한 hop 수나 범위, 방향을 지정할 수 있습니다.

```sql
GO 2 STEPS FROM 1 OVER knows;
GO 1..3 STEPS FROM 1 OVER knows;
GO FROM 1 OVER knows REVERSELY;
GO FROM 1 OVER knows BIDIRECT;
```

Edge 또는 도착 버텍스 속성을 필터링하고 투영합니다.

```sql
GO FROM 1 OVER knows
WHERE knows.since >= 2020
YIELD src(edge) AS src, dst(edge) AS dst, knows.since AS since;

GO FROM 1 OVER knows
YIELD $$.person.name AS friend_name;
```

기본 설정에서는 20 step을 넘는 GO 범위를 실행 가드가 거부합니다.

## MATCH

Vertex와 edge 패턴을 매칭합니다.

```sql
MATCH (p:person) RETURN id(p) AS vid, p.person.name AS name;

MATCH (a:person)-[e:knows]->(b:person)
WHERE a.person.name == "Alice" AND b.person.age >= 20
RETURN b.person.name AS friend;
```

리터럴 속성 map과 가변 길이 edge를 지원합니다.

```sql
MATCH (p:person {name: "Alice"}) RETURN p;
MATCH (a:person)-[:knows*1..3]->(b:person) RETURN id(b) AS vid;
```

쉼표로 나눈 여러 패턴은 공유 변수로 join합니다. 현재 MATCH 경로는
`OPTIONAL MATCH`, `GROUP BY`, `ORDER BY`, `LIMIT`, `OFFSET`도 지원합니다.

집계 함수로 `COUNT`, `SUM`, `AVG`, `MIN`, `MAX`를 사용할 수 있습니다.

```sql
MATCH (p:person)
RETURN p.person.city AS city, COUNT(*) AS people
GROUP BY p.person.city
ORDER BY people DESC
LIMIT 10;
```

온톨로지 계층을 인식하는 membership에는 `is_a`를 사용합니다.

```sql
MATCH (n:dog) WHERE is_a(n, "animal") RETURN id(n) AS vid;
```

## LOOKUP

현재 `LOOKUP`은 tag를 대상으로 합니다.

```sql
LOOKUP ON person WHERE person.name == "Alice";
LOOKUP ON person WHERE person.age >= 21 YIELD person.name, person.age LIMIT 20;
```

Tag 인덱스가 조건을 가속할 수 있지만 인덱스가 없으면 제한된 대체 스캔을 사용할
수 있습니다. 기본 대체 스캔 한도는 100,000행이며 설정할 수 있습니다. Edge
타입에 `LOOKUP`을 실행하면 edge 조회 대신 오류를 반환합니다.

## FIND 경로

```sql
FIND SHORTEST PATH FROM 1 TO 3 OVER knows;
FIND SHORTEST PATH FROM 1 TO 3 OVER road WEIGHT BY distance;
FIND SHORTEST PATH FROM 1 TO 3 OVER knows BIDIRECT UPTO 5 STEPS;
FIND ALL SHORTEST PATHS FROM 1 TO 3 OVER knows UPTO 5 STEPS;
```

`OVER` 대상은 edge 타입 하나 또는 `*`이며 쉼표로 나눈 edge 목록이 아닙니다.
경로 순회와 all-shortest-path 열거에는 executor 자원 한도가 적용됩니다.

## RECOMMEND

구조 모드는 outgoing 이웃의 Jaccard overlap으로 후보 순위를 정합니다.

```sql
RECOMMEND SIMILAR TO 1001 OVER has_brand, in_category LIMIT 5;
RECOMMEND SIMILAR TO 1001 OVER * WHERE channel != seed.channel LIMIT 5;
```

임베딩 모드는 숫자 리스트 속성의 cosine similarity를 사용합니다.

```sql
RECOMMEND SIMILAR TO 1001 BY EMBEDDING embedding LIMIT 5;
```

쿼리별 가중치로 두 신호를 섞을 수 있습니다.

```sql
RECOMMEND SIMILAR TO 1001
BLEND EMBEDDING embedding 0.7 OVER has_brand, in_category 0.3
LIMIT 5;
```

기본 추천 limit은 10입니다. 벡터 수가 적으면 exact scan을, 구현 임계치를 넘으면
영속 HNSW 인덱스를 사용하며 현재 변경 경로가 이를 유지합니다.

## 복합 쿼리와 검사

결과를 변수에 바인딩하고 뒤 절에서 VID 컬럼을 사용합니다.

```sql
$first = GO FROM 1 OVER knows YIELD dst(edge) AS vid;
GO FROM $first.vid OVER knows YIELD dst(edge) AS vid;
```

복합 쿼리 전체를 한 요청으로 보내세요. `|` 파이프 문법은 없습니다.

`EXPLAIN`은 실행하지 않고 논리 계획을 보여 주며 `PROFILE`은 문을 실행하고
operator metric을 붙입니다.

```sql
EXPLAIN MATCH (p:person) RETURN p;
PROFILE GO FROM 1 OVER knows;
```

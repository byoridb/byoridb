# 데이터 쿼리

[English](../../../guide/ngql/dql.html) | **한국어**

query statement는 인증 session에서 `USE`로 선택한 space를 사용합니다.

## FETCH PROP

space에 선택한 VID type으로 vertex를 조회합니다.

```sql
FETCH PROP ON person 1;
FETCH PROP ON person 1, 2, 3;
FETCH PROP ON * 1;
FETCH PROP ON person "alice";
```

여러 VID를 한 문장에 넣으면 내부적으로 한 번의 `batch_get`을 사용합니다. 다만 수백
개 ID를 넣는 LDBC 규모의 실용 한도와 응답 크기 기준은 아직 확정되지 않았습니다.
HTTP query 1 MiB 제한 안에서 사용하고 실제 workload의 응답 크기를 측정하세요. 대규모
batch 정확성·성능 검증은 [이슈 #10](https://github.com/byoridb/byoridb/issues/10)에서
추적합니다.

endpoint pair로 edge를 조회합니다.

```sql
FETCH PROP ON knows 1->2;
FETCH PROP ON * 1->2;
FETCH PROP ON knows "alice"->"bob";
```

현재 temporal read는 vertex와 edge 모두 epoch-millisecond timestamp를 받습니다.

```sql
FETCH PROP ON person 1 AS OF 1785283200000;
FETCH PROP ON knows 1->2 AS OF 1785283200000;
```

`AS OF`는 asserted history를 조회하며 같은 timestamp를 valid time과 transaction time에
적용합니다. Historical `MATCH`, `GO`, `RECOMMEND`는 구현되지 않았고 과거 inferred
fact의 재구성도 보장하지 않습니다.

## GO

하나 이상의 outgoing edge type을 순회합니다.

```sql
GO FROM 1 OVER knows;
GO FROM 1 OVER knows, follows;
GO FROM 1 OVER *;
```

정확한 hop 수나 범위와 방향을 지정할 수 있습니다.

```sql
GO 2 STEPS FROM 1 OVER knows;
GO 1..3 STEPS FROM 1 OVER knows;
GO FROM 1 OVER knows REVERSELY;
GO FROM 1 OVER knows BIDIRECT;
```

edge 또는 destination 속성으로 filter와 projection을 구성합니다.

```sql
GO FROM 1 OVER knows
WHERE knows.since >= 2020
YIELD src(edge) AS src, dst(edge) AS dst, knows.since AS since;

GO FROM 1 OVER knows
YIELD $$.person.name AS friend_name;
```

destination-property projection은 동작하지만 현재 executor는 결과 행마다 destination
vertex를 개별 조회합니다. 이 내부 조회의 batching과 대규모 회귀 테스트도
[이슈 #10](https://github.com/byoridb/byoridb/issues/10)의 남은 작업입니다.

기본 execution guard는 20 step을 넘는 GO range를 거부합니다.

## MATCH

vertex와 edge pattern을 match합니다.

```sql
MATCH (p:person) RETURN id(p) AS vid, p.person.name AS name;

MATCH (a:person)-[e:knows]->(b:person)
WHERE a.person.name == "Alice" AND b.person.age >= 20
RETURN b.person.name AS friend;
```

literal property map과 variable-length edge도 지원합니다.

```sql
MATCH (p:person {name: "Alice"}) RETURN p;
MATCH (a:person)-[:knows*1..3]->(b:person) RETURN id(b) AS vid;
```

comma-separated pattern은 공유 variable로 join합니다. 현재 MATCH path는
`OPTIONAL MATCH`, `GROUP BY`, `ORDER BY`, `LIMIT`, `OFFSET`도 지원합니다.

aggregate는 `COUNT`, `SUM`, `AVG`, `MIN`, `MAX`를 지원합니다.

```sql
MATCH (p:person)
RETURN p.person.city AS city, COUNT(*) AS people
GROUP BY p.person.city
ORDER BY people DESC
LIMIT 10;
```

ontology-aware membership는 `is_a`를 사용합니다.

```sql
MATCH (n:dog) WHERE is_a(n, "animal") RETURN id(n) AS vid;
```

## LOOKUP

현재 `LOOKUP`은 tag를 대상으로 합니다.

```sql
LOOKUP ON person WHERE person.name == "Alice";
LOOKUP ON person WHERE person.age >= 21 YIELD person.name, person.age LIMIT 20;
```

indexed tag 속성의 동등 조건은 secondary index를 사용할 수 있습니다. 그러나 `>`,
`>=`, `<`, `<=` 범위 조건은 해당 속성에 index가 있어도 bounded full scan으로
fallback합니다. index range scan은
[이슈 #1](https://github.com/byoridb/byoridb/issues/1)에서 추적합니다. 기본 fallback
scan 상한은 100,000행이며 설정으로 바꿀 수 있습니다. edge type `LOOKUP`은 현재
조회하지 않고 명시적 오류를 반환합니다. 실제 access path는 `EXPLAIN` 또는 `PROFILE`로
확인하세요.

## FIND path

```sql
FIND SHORTEST PATH FROM 1 TO 3 OVER knows;
FIND SHORTEST PATH FROM 1 TO 3 OVER road WEIGHT BY distance;
FIND SHORTEST PATH FROM 1 TO 3 OVER knows BIDIRECT UPTO 5 STEPS;
FIND ALL SHORTEST PATHS FROM 1 TO 3 OVER knows UPTO 5 STEPS;
```

`OVER`는 하나의 edge type 또는 `*`를 받으며 comma-separated edge list는 받지 않습니다.
path traversal과 all-shortest-path 열거에는 executor resource cap이 적용됩니다.

## RECOMMEND

structural mode는 outgoing neighbor의 Jaccard overlap으로 candidate를 정렬합니다.

```sql
RECOMMEND SIMILAR TO 1001 OVER has_brand, in_category LIMIT 5;
RECOMMEND SIMILAR TO 1001 OVER * WHERE channel != seed.channel LIMIT 5;
```

embedding mode는 numeric-list property의 cosine similarity를 사용합니다.

```sql
RECOMMEND SIMILAR TO 1001 BY EMBEDDING embedding LIMIT 5;
```

두 signal은 query-time weight로 섞을 수 있습니다.

```sql
RECOMMEND SIMILAR TO 1001
BLEND EMBEDDING embedding 0.7 OVER has_brand, in_category 0.3
LIMIT 5;
```

기본 recommendation limit은 10입니다. 작은 vector collection은 exact scan을 사용하고
implementation threshold를 넘으면 current mutation path가 유지하는 persisted HNSW
index를 사용합니다.

## Compound query와 검사

결과를 variable에 bind한 뒤 후속 clause에서 VID column을 사용할 수 있습니다.

```sql
$first = GO FROM 1 OVER knows YIELD dst(edge) AS vid;
GO FROM $first.vid OVER knows YIELD dst(edge) AS vid;
```

compound query 전체를 한 request로 보내세요. `|` pipeline 문법은 없습니다.

`EXPLAIN`은 실행 없이 logical plan을 보여 주고 `PROFILE`은 실행하면서 operator
measurement를 기록합니다.

```sql
EXPLAIN MATCH (p:person) RETURN p;
PROFILE GO FROM 1 OVER knows;
```

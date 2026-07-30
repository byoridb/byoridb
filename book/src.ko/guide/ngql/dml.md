# 데이터 조작

[English](../../../guide/ngql/dml.html) | **한국어**

그래프 데이터를 변경하기 전에 space를 선택하고 사용할 tag 또는 edge schema를
만드세요. 현재 실행 경로는 정수 VID를 요구합니다.

## Vertex 삽입

```sql
INSERT VERTEX person(name, age) VALUES 1:("Alice", 30);
```

한 문장에 여러 vertex를 넣으면 current view와 history가 하나의 storage transaction에
기록됩니다.

```sql
INSERT VERTEX person(name, age) VALUES
    1:("Alice", 30),
    2:("Bob", 25),
    3:("Carol", 28);
```

명시한 속성만 전달됩니다. 존재하지 않는 속성명과 명백히 맞지 않는 scalar 타입은
거부됩니다. 현재 INSERT 경로는 schema default로 누락 속성을 채우지 않으므로 필요한
값을 직접 전달하거나 shape로 강제하세요.

숫자 list literal은 recommendation embedding 등에 사용할 수 있습니다.

```sql
CREATE TAG product(name STRING, embedding STRING NULL);
INSERT VERTEX product(name, embedding) VALUES
    1001:("Widget", [0.12, -0.04, 0.88]);
```

현재 DDL parser에는 list property type이 없습니다. 속성명이 존재하면 composite value
검증은 느슨하므로 위 예시는 `embedding`에 임시 scalar 선언을 사용합니다.

## Edge 삽입

기본 ranking은 `0`입니다.

```sql
INSERT EDGE knows(since) VALUES 1->2:(2020), 2->3:(2021);
INSERT EDGE empty_relation() VALUES 1->3:();
```

같은 type과 endpoint 사이에 여러 edge를 저장하려면 `@<rank>`를 사용합니다.

```sql
INSERT EDGE knows(since) VALUES 1->2@1:(2020), 1->2@2:(2024);
```

semantic edge 선언이 있으면 asserted edge 삽입 뒤 추가 edge나 class membership이
materialize될 수 있습니다.

## Vertex 갱신

```sql
UPDATE VERTEX ON person 1 SET age = 31;
UPDATE VERTEX ON person 1 SET name = "Alicia", age = 32;
```

`WHEN`은 현재 vertex를 기준으로 평가되며 false이면 아무것도 바꾸지 않습니다.

```sql
UPDATE VERTEX ON person 1 SET age = 33 WHEN age == 32;
```

현재 assignment planning은 literal과 list 값만 받습니다. `SET score = score + 1` 같은
산술 assignment는 동작하지 않습니다.

`WHEN`이 없는 `UPDATE VERTEX`는 upsert로 동작해 VID가 없으면 vertex/tag를 만듭니다.
update-only가 필요하면 먼저 존재를 확인하거나 현재 데이터에 묶인 참인 `WHEN` 조건을
사용하세요.

`UPDATE EDGE`는 parser가 받지만 현재 plan은 edge identity를 보존하지 못해 실행에
실패합니다. 전용 edge-update executor가 구현되기 전에는 삭제 후 다시 삽입하세요.

## Vertex 삭제

```sql
DELETE VERTEX 1;
DELETE VERTEX 1, 2, 3;
DELETE VERTEX 7 WHERE status == "inactive";
```

vertex 삭제는 vertex record, tag index, embedding과 적용 가능한 inferred
materialization을 정리합니다. 그러나 연결된 모든 asserted edge의 cascade 삭제는 현재
보장하지 않습니다. 참조 정리가 중요하면 edge를 명시적으로 삭제하세요.

되돌릴 수 없는 `sameAs` merge에 참여한 vertex는 삭제할 수 없습니다.

## Edge 삭제

```sql
DELETE EDGE knows 1->2;
DELETE EDGE knows 1->2@1, 2->3@2;
```

asserted semantic edge를 삭제하면 현재 inference-maintenance 경로가 inferred 결과를
철회합니다. canonical vertex를 unmerge할 수 없으므로 `sameAs` edge는 삭제할 수 없습니다.

## 원자성과 history

- 여러 행을 넣는 vertex/edge INSERT는 current-view record와 history version을 한 storage
  transaction에 적용합니다.
- UPDATE와 DELETE도 current view와 temporal history를 함께 기록합니다.
- 서로 다른 nGQL 요청은 cross-statement transaction이 아닙니다.
- 세미콜론으로 묶은 compound statement도 순차 실행일 뿐 transactional하지 않습니다.
  뒤 clause가 실패해도 이미 성공한 clause는 rollback되지 않습니다.
- 각 version은 server의 epoch-millisecond transaction time을 받습니다. 현재 temporal
  interface에서는 하나의 `AS OF` 값을 valid time과 transaction time 양쪽에 적용합니다.

vertex와 edge의 `FETCH PROP ... AS OF`는 [데이터 쿼리](./dql.md)를 참고하세요.

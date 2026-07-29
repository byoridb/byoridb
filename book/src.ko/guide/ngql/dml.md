[English](../../../guide/ngql/dml.html)

# 데이터 변경

그래프 데이터를 변경하기 전에 스페이스를 선택하고 참조할 tag 또는 edge 스키마를
만드세요. 현재 실행 경로는 정수 버텍스 ID를 요구합니다.

## 버텍스 삽입

```sql
INSERT VERTEX person(name, age) VALUES 1:("Alice", 30);
```

한 문에서 여러 버텍스를 하나의 current-view/history transaction으로 batch할 수
있습니다.

```sql
INSERT VERTEX person(name, age) VALUES
    1:("Alice", 30),
    2:("Bob", 25),
    3:("Carol", 28);
```

이름을 나열한 속성만 전달합니다. 알 수 없는 속성 이름과 명백히 호환되지 않는
scalar 타입은 거부됩니다. 현재 INSERT 경로는 스키마 기본값으로 생략한 속성을
채우지 않으므로 필요한 값을 명시적으로 전달하거나 shape로 강제하세요.

숫자 리스트 리터럴도 허용되며 추천 임베딩에 사용할 수 있습니다.

```sql
CREATE TAG product(name STRING, embedding STRING NULL);
INSERT VERTEX product(name, embedding) VALUES
    1001:("Widget", [0.12, -0.04, 0.88]);
```

현재 DDL 파서에는 list 속성 타입이 없습니다. 속성 이름이 존재하면 composite 값을
느슨하게 검증하므로 예시는 `embedding`에 임시 scalar 선언을 사용합니다.

## Edge 삽입

기본 ranking은 `0`입니다.

```sql
INSERT EDGE knows(since) VALUES 1->2:(2020), 2->3:(2021);
INSERT EDGE empty_relation() VALUES 1->3:();
```

같은 타입과 endpoint를 가진 병렬 edge는 `@<rank>`로 저장합니다.

```sql
INSERT EDGE knows(since) VALUES 1->2@1:(2020), 1->2@2:(2024);
```

시맨틱 edge 선언에 따라 asserted edge 삽입 후 inferred edge나 class membership이
추가로 구체화될 수 있습니다.

## 버텍스 수정

```sql
UPDATE VERTEX ON person 1 SET age = 31;
UPDATE VERTEX ON person 1 SET name = "Alicia", age = 32;
```

`WHEN`은 현재 버텍스를 기준으로 평가하며 false이면 아무것도 변경하지 않습니다.

```sql
UPDATE VERTEX ON person 1 SET age = 33 WHEN age == 32;
```

현재 할당 계획은 리터럴과 리스트 값을 처리합니다. `SET score = score + 1` 같은
산술 할당은 동작하지 않습니다.

`WHEN`이 없는 `UPDATE VERTEX`는 upsert처럼 동작해 VID가 없으면 버텍스와 tag를
생성합니다. 애플리케이션에 update-only 동작이 필요하면 먼저 존재 여부를
확인하거나 현재 데이터에 연결된 참 `WHEN` 조건을 사용하세요.

`UPDATE EDGE`는 파싱되지만 실행 계획이 edge 식별자를 버려 현재 실패합니다.
전용 edge update executor가 구현될 때까지 edge를 삭제한 뒤 다시 삽입하세요.

## 버텍스 삭제

```sql
DELETE VERTEX 1;
DELETE VERTEX 1, 2, 3;
DELETE VERTEX 7 WHERE status == "inactive";
```

버텍스 삭제는 버텍스 레코드, tag 인덱스 항목, 임베딩, 적용 가능한 추론 구체화를
제거합니다. 모든 asserted incident edge의 cascade 삭제는 현재 보장하지 않습니다.
참조 정리가 중요하면 edge를 명시적으로 삭제하세요.

되돌릴 수 없는 `sameAs` merge에 포함된 버텍스는 삭제할 수 없습니다.

## Edge 삭제

```sql
DELETE EDGE knows 1->2;
DELETE EDGE knows 1->2@1, 2->3@2;
```

Asserted semantic edge를 삭제하면 현재 추론 유지 경로가 도출 결과를 철회합니다.
Canonical vertex를 unmerge하는 기능이 없으므로 `sameAs` edge는 삭제할 수 없습니다.

## 원자성과 이력

- 여러 행의 vertex/edge INSERT는 현재 뷰 레코드와 이력 버전을 한 저장소
  트랜잭션으로 적용합니다.
- 현재 UPDATE와 DELETE 경로도 현재 뷰와 temporal history를 함께 기록합니다.
- 서로 다른 nGQL 요청은 cross-statement 트랜잭션에 포함되지 않습니다.
- 세미콜론으로 묶은 복합 문도 순차 실행일 뿐 트랜잭션이 아닙니다. 뒤 절이
  실패해도 앞에서 성공한 절은 rollback되지 않습니다.
- 각 버전은 서버의 epoch millisecond transaction time을 받습니다. 현재 temporal
  인터페이스는 하나의 `AS OF` 값을 valid-time과 transaction-time 두 축에 함께
  적용합니다.

Vertex와 edge의 `FETCH PROP ... AS OF` 읽기는 [데이터 조회](./dql.md)를 보세요.

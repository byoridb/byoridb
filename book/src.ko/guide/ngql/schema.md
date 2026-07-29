[English](../../../guide/ngql/schema.html)

# 스키마

이 페이지의 스키마 문을 실행하기 전에 스페이스를 선택하세요.

```sql
USE social;
```

## Tag

Tag는 버텍스 타입과 그 속성을 정의합니다.

```sql
CREATE TAG person(
    name STRING DEFAULT "unknown",
    age INT64 NULL,
    active BOOL DEFAULT true
);
```

`IF NOT EXISTS`를 지원합니다.

```sql
CREATE TAG IF NOT EXISTS person(name STRING, age INT64);
```

지원되는 속성 선언은 `BOOL`, `INT8`, `INT16`, `INT32`, `INT64`(`INT`는 별칭),
`FLOAT`, `DOUBLE`, `STRING`, `TIMESTAMP`, `DATE`, `TIME`, `DATETIME`입니다.
`CREATE TAG`/`CREATE EDGE`에서는 nullable 속성에 `NULL`을 붙이며 생략하면
non-nullable로 기록합니다. `NOT NULL` 표기는 create-property 파서가 아니라
`ALTER ... ADD/CHANGE`에서 지원합니다.

기본값과 nullability는 스키마 메타데이터에 저장되지만 현재 INSERT 경로가 생략된
기본값을 합성하거나 누락된 모든 non-null 필드를 강제하지는 않습니다. 필요한 값을
명시적으로 전달하거나 shape로 강제하세요.

Tag를 조회하거나 제거합니다.

```sql
SHOW TAGS;
DESCRIBE TAG person;
SHOW CREATE TAG person;
DROP TAG IF EXISTS person;
```

`DESC TAG person`은 `DESCRIBE TAG person`의 별칭입니다.

## Tag 변경

파서와 로컬 executor는 스키마 컬럼 추가, 삭제, 변경을 지원합니다.

```sql
ALTER TAG person ADD (email STRING NULL);
ALTER TAG person DROP (email);
ALTER TAG person CHANGE (age INT32 NULL);
```

`ADD` 컬럼은 기본적으로 nullable입니다. 새 non-null 컬럼에는 기본값을
지정하세요.

```sql
ALTER TAG person ADD (verified BOOL NOT NULL DEFAULT false);
```

스키마 변경은 스키마 메타데이터를 갱신합니다. 변경된 타입에 의존하기 전에
마이그레이션을 계획하고 기존 행을 검증하세요.

## Edge 타입

```sql
CREATE EDGE knows(since INT64, strength DOUBLE DEFAULT 1.0);
CREATE EDGE IF NOT EXISTS follows(since TIMESTAMP);
```

Edge 스키마 조회와 변경은 tag 문법과 대응합니다.

```sql
SHOW EDGES;
DESCRIBE EDGE knows;
SHOW CREATE EDGE knows;
ALTER EDGE knows ADD (source STRING NULL);
DROP EDGE IF EXISTS knows;
```

## 시맨틱 edge 선언

ByoriDB는 온톨로지 함의를 현재 그래프 뷰에 구체화할 수 있습니다. edge 속성
목록 뒤에 다음 절을 사용할 수 있습니다.

| 절 | 효과 |
| --- | --- |
| `TRANSITIVE` | `a -> b`, `b -> c`에서 `a -> c` 도출 |
| `SYMMETRIC` | 반대 방향 edge 도출 |
| `INVERSE OF other` | 대응하는 역방향 `other` edge 구체화 |
| `SUBPROPERTY OF other` | 같은 endpoint를 `other` 타입으로 구체화 |
| `EQUIVALENT TO other` | 두 property를 구체화 관점에서 동치로 취급 |
| `CHAIN first, second` | property chain으로 선언된 edge 도출 |
| `DOMAIN class` | 출발 버텍스의 class 추론 |
| `RANGE class` | 도착 버텍스의 class 추론 |

참조하는 edge 타입과 class는 먼저 존재해야 합니다.

```sql
CREATE CLASS city(name STRING);
CREATE EDGE related();
CREATE EDGE parent();
CREATE EDGE ancestor() TRANSITIVE SUBPROPERTY OF related;
CREATE EDGE child() INVERSE OF parent;
CREATE EDGE born_in() RANGE city;
```

삽입은 구체화 closure를 확장합니다. 현재 `DELETE EDGE`는 점진적 추론 철회를,
`DELETE VERTEX`는 스페이스 재구체화를 수행합니다. 과거 `AS OF` 읽기는 asserted
DML 버전을 중심으로 설계되었으므로 과거의 모든 inferred fact 재현에 의존하면
안 됩니다.

기록된 inferred edge의 도출 근거는 다음과 같이 조회합니다.

```sql
WHY 1 -> 3 OVER ancestor;
```

예약된 `sameAs` edge 이름은 edge 삽입 시 되돌릴 수 없는 canonical vertex
merge를 수행합니다. unmerge가 없으므로 해당 `sameAs` edge나 관련 버텍스 삭제는
거부됩니다.

## Class와 일관성

Class는 호환 tag 스키마도 만들며 계층, 동치, 서로소 관계를 선언할 수 있습니다.

```sql
CREATE CLASS animal(name STRING);
CREATE CLASS pet(owner STRING);
CREATE CLASS dog(breed STRING) SUBCLASS OF animal, pet;
CREATE CLASS person_kind(name STRING);
CREATE CLASS human(name STRING) EQUIVALENT TO person_kind;
CREATE CLASS building() DISJOINT WITH animal;
```

참조하는 superclass와 equivalent class는 먼저 존재해야 합니다. 모델을 조회하고
검증합니다.

```sql
SHOW CLASSES;
DESCRIBE CLASS dog;
CHECK CONSISTENCY;
DROP CLASS IF EXISTS dog;
```

계층을 인식하는 class membership 조회에는 `MATCH` 조건의
`is_a(vertex, "class")`를 사용합니다.

## Shape

Shape는 대상 class에 필수 속성, 데이터 타입, 값 조건 검사를 추가합니다.

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

쓰기 작업은 해당 shape로 검증되며 `CHECK SHAPE`는 그래프에 이미 존재하는 위반을
보고합니다.

## 인덱스

선택한 스페이스에서 보조 인덱스를 생성하고 조회합니다.

```sql
CREATE TAG INDEX person_name_idx ON person(name);
CREATE EDGE INDEX knows_since_idx ON knows(since);

SHOW TAG INDEXES;
SHOW EDGE INDEXES;
DESCRIBE TAG INDEX person_name_idx;
DESCRIBE EDGE INDEX knows_since_idx;
```

현재 drop 문법은 `INDEX`를 스키마 종류보다 먼저 둡니다.

```sql
DROP INDEX TAG IF EXISTS person_name_idx;
DROP INDEX EDGE IF EXISTS knows_since_idx;
```

Tag 인덱스는 `LOOKUP`을 가속할 수 있고 현재 INSERT, UPDATE, DELETE 경로에서
유지됩니다. Edge index DDL은 존재하지만 edge 타입 `LOOKUP`은 아직 연결되지
않았습니다.

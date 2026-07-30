# 스키마

[English](../../../guide/ngql/schema.html) | **한국어**

이 페이지의 schema 문을 실행하기 전에 space를 선택하세요.

```sql
USE social;
```

## Tag

tag는 vertex type과 속성을 정의합니다.

```sql
CREATE TAG person(
    name STRING DEFAULT "unknown",
    age INT64 NULL,
    active BOOL DEFAULT true
);

CREATE TAG IF NOT EXISTS person(name STRING, age INT64);
```

지원하는 property 선언은 `BOOL`, `INT8`, `INT16`, `INT32`, `INT64`(`INT` alias),
`FLOAT`, `DOUBLE`, `STRING`, `TIMESTAMP`, `DATE`, `TIME`, `DATETIME`입니다.
`CREATE TAG`/`CREATE EDGE`에서 nullable 속성에는 `NULL`을 붙입니다. 생략하면
non-nullable metadata로 기록됩니다. `NOT NULL` spelling은 create-property parser가
아니라 `ALTER ... ADD/CHANGE`에서 지원합니다.

default와 nullability는 schema metadata에 저장되지만 현재 INSERT 경로는 누락된 값을
default로 채우거나 모든 누락 non-null field를 강제하지 않습니다. 필요한 값을 직접
전달하거나 shape로 강제하세요.

```sql
SHOW TAGS;
DESCRIBE TAG person;
SHOW CREATE TAG person;
DROP TAG IF EXISTS person;
```

`DESC TAG person`은 `DESCRIBE TAG person`의 alias입니다.

## Tag 변경

parser와 local executor는 column 추가, 삭제, 변경을 지원합니다.

```sql
ALTER TAG person ADD (email STRING NULL);
ALTER TAG person DROP (email);
ALTER TAG person CHANGE (age INT32 NULL);
```

`ADD` column은 기본적으로 nullable입니다. 새 non-nullable column에는 default를
제공하세요.

```sql
ALTER TAG person ADD (verified BOOL NOT NULL DEFAULT false);
```

schema 변경은 metadata를 바꿉니다. 기존 row가 변경된 type을 만족한다고 가정하지 말고
migration과 검증을 계획하세요.

## Edge type

```sql
CREATE EDGE knows(since INT64, strength DOUBLE DEFAULT 1.0);
CREATE EDGE IF NOT EXISTS follows(since TIMESTAMP);

SHOW EDGES;
DESCRIBE EDGE knows;
SHOW CREATE EDGE knows;
ALTER EDGE knows ADD (source STRING NULL);
DROP EDGE IF EXISTS knows;
```

## Semantic edge 선언

ByoriDB는 ontology entailment를 현재 graph view에 materialize할 수 있습니다. 다음
clause를 edge property list 뒤에 붙일 수 있습니다.

| Clause | 효과 |
| --- | --- |
| `TRANSITIVE` | `a -> b`, `b -> c`에서 `a -> c` 도출 |
| `SYMMETRIC` | 역방향 edge 도출 |
| `INVERSE OF other` | 대응하는 역방향 `other` edge materialize |
| `SUBPROPERTY OF other` | 같은 endpoint를 `other` type으로 materialize |
| `EQUIVALENT TO other` | 두 property를 동치로 materialize |
| `CHAIN first, second` | 두 property chain에서 선언한 edge 도출 |
| `DOMAIN class` | source vertex의 class 추론 |
| `RANGE class` | destination vertex의 class 추론 |

참조하는 edge type과 class는 먼저 존재해야 합니다.

```sql
CREATE CLASS city(name STRING);
CREATE EDGE related();
CREATE EDGE parent();
CREATE EDGE ancestor() TRANSITIVE SUBPROPERTY OF related;
CREATE EDGE child() INVERSE OF parent;
CREATE EDGE born_in() RANGE city;
```

삽입은 materialized closure를 확장합니다. 현재 `DELETE EDGE`는 incremental inference
retraction을 수행하고 `DELETE VERTEX`는 space를 rematerialize합니다. 역사적 `AS OF`
읽기는 asserted DML version을 중심으로 설계됐으므로 모든 과거 inferred fact가 재현된다고
가정하지 마세요.

```sql
WHY 1 -> 3 OVER ancestor;
```

예약된 `sameAs` edge는 삽입 시 canonical vertex를 되돌릴 수 없게 merge합니다. unmerge가
구현되지 않아 `sameAs` edge와 관련 vertex 삭제는 거부됩니다.

## Class와 일관성

class는 호환 tag schema를 함께 만들며 hierarchy, equivalence, disjointness를 선언할 수
있습니다.

```sql
CREATE CLASS animal(name STRING);
CREATE CLASS pet(owner STRING);
CREATE CLASS dog(breed STRING) SUBCLASS OF animal, pet;
CREATE CLASS person_kind(name STRING);
CREATE CLASS human(name STRING) EQUIVALENT TO person_kind;
CREATE CLASS building() DISJOINT WITH animal;
```

참조하는 superclass와 equivalent class는 먼저 존재해야 합니다.

```sql
SHOW CLASSES;
DESCRIBE CLASS dog;
CHECK CONSISTENCY;
DROP CLASS IF EXISTS dog;
```

hierarchy-aware membership는 `MATCH` predicate에서 `is_a(vertex, "class")`로 조회합니다.

## Shape

shape는 대상 class에 required-property, datatype, value-predicate 검사를 추가합니다.

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

write는 적용 가능한 shape로 검증되며 `CHECK SHAPE`는 이미 존재하는 위반을 보고합니다.

## Index

```sql
CREATE TAG INDEX person_name_idx ON person(name);
CREATE EDGE INDEX knows_since_idx ON knows(since);

SHOW TAG INDEXES;
SHOW EDGE INDEXES;
DESCRIBE TAG INDEX person_name_idx;
DESCRIBE EDGE INDEX knows_since_idx;
```

현재 drop 문법은 `INDEX`가 schema kind보다 앞에 옵니다.

```sql
DROP INDEX TAG IF EXISTS person_name_idx;
DROP INDEX EDGE IF EXISTS knows_since_idx;
```

tag index는 `LOOKUP`의 동등 조건을 가속하며 현재 INSERT, UPDATE, DELETE에서 유지됩니다.
range predicate는 아직 index range scan을 사용하지 않습니다. edge index DDL은 존재하지만
edge type `LOOKUP`은 현재 연결되지 않았습니다.

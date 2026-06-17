# 스키마 정의

태그(버텍스 타입)와 엣지로 그래프의 구조를 정의합니다.

## 태그 (버텍스 타입)

### CREATE TAG

```sql
CREATE TAG <tag_name> (
    <property_name> <data_type> [NULL | NOT NULL] [DEFAULT <value>],
    ...
);
```

**예시:**

```sql
CREATE TAG person(name STRING, age INT64);
CREATE TAG player(name STRING NOT NULL, score INT64 DEFAULT 0);
CREATE TAG product(
    name STRING,
    price DOUBLE,
    in_stock BOOL DEFAULT true,
    created_at TIMESTAMP
);
```

### ALTER TAG

기존 태그에 새 속성을 추가합니다(온라인 스키마 변경):

```sql
ALTER TAG <tag_name> ADD (<property_name> <data_type> [NULL | DEFAULT <value>]);
```

**예시:**

```sql
-- Add nullable column
ALTER TAG person ADD (email STRING NULL);

-- Add column with default value
ALTER TAG player ADD (level INT64 DEFAULT 1);
```

> **참고:** 새 컬럼은 nullable(`NULL`)이거나 기본값을 가져야 합니다. 기존 버텍스는 새 속성에 대해 `NULL` 또는 기본값을 반환합니다.

### SHOW TAGS

```sql
SHOW TAGS;
```

### DESCRIBE TAG

```sql
DESCRIBE TAG person;
DESC TAG person;
```

### DROP TAG

```sql
DROP TAG <tag_name>;
DROP TAG person;
DROP TAG IF EXISTS person;
```

## 엣지 타입

### CREATE EDGE

```sql
CREATE EDGE <edge_name> (
    <property_name> <data_type> [NULL | NOT NULL] [DEFAULT <value>],
    ...
);
```

**예시:**

```sql
CREATE EDGE knows();
CREATE EDGE follow(since TIMESTAMP);
CREATE EDGE purchase(
    quantity INT64 DEFAULT 1,
    price DOUBLE,
    purchased_at DATETIME
);
```

#### 시맨틱 관계 타입 (온톨로지 추론)

엣지 타입에 온톨로지 시맨틱을 선언하면, INSERT 시 함의되는 엣지가 자동으로
도출·저장되어(forward-chaining materialization) MATCH/GO에서 추론 없이 그대로
조회됩니다. 속성 목록 뒤에 다음 절을 (순서 무관, 복수) 붙입니다:

```sql
CREATE EDGE <edge_name> (<properties>)
    [TRANSITIVE]              -- (a)-p->(b) ∧ (b)-p->(c) ⟹ (a)-p->(c)
    [SYMMETRIC]               -- (a)-p->(b) ⟹ (b)-p->(a)
    [INVERSE OF <edge>]       -- (a)-p->(b) ⟹ (b)-q->(a) (양방향)
    [SUBPROPERTY OF <edge>]   -- (a)-p->(b) ⟹ (a)-q->(b)
    [DOMAIN <class>]          -- (a)-p->(b) ⟹ a is-a <class> (주어 타입 추론)
    [RANGE <class>];          -- (a)-p->(b) ⟹ b is-a <class> (목적어 타입 추론)
```

`DOMAIN`/`RANGE`는 정점의 클래스(타입)를 추론합니다. 추론된 타입은 `is_a(...)`로
조회됩니다(서브클래스 계층도 자동 확장).

```sql
-- bornIn의 목적어는 City: alice-[bornIn]->seoul 삽입 시 seoul이 City로 추론
CREATE CLASS city();
CREATE EDGE bornIn() RANGE city;
-- 이후 MATCH (n) WHERE is_a(n, "city") 가 seoul을 매칭
```

```sql
-- ancestor는 추이적: 1->2, 2->3 삽입 시 1->3 자동 도출
CREATE EDGE ancestor() TRANSITIVE;

-- knows는 대칭: 1->2 삽입 시 2->1 자동 도출
CREATE EDGE knows() SYMMETRIC;

-- child INVERSE OF parent: child 1->2 ⟹ parent 2->1 (그 반대도)
CREATE EDGE parent();
CREATE EDGE child() INVERSE OF parent;

-- knows ⊑ related: knows 1->2 ⟹ related 1->2
CREATE EDGE related();
CREATE EDGE knows2() SUBPROPERTY OF related;
```

> **주의 (현재 단계):** materialization은 **삽입 전용**입니다. 시맨틱은 데이터를
> 넣기 *전*에 선언하세요(`CREATE EDGE ... TRANSITIVE` → `INSERT EDGE`). 삭제는
> 아직 추론을 철회하지 않습니다(후속 단계). `INVERSE OF`/`SUBPROPERTY OF` 대상
> 엣지 타입은 미리 존재해야 합니다.

### ALTER EDGE

기존 엣지 타입에 새 속성을 추가합니다:

```sql
ALTER EDGE <edge_name> ADD (<property_name> <data_type> [NULL | DEFAULT <value>]);
```

**예시:**

```sql
ALTER EDGE follow ADD (weight DOUBLE NULL);
ALTER EDGE purchase ADD (discount DOUBLE DEFAULT 0.0);
```

### SHOW EDGES

```sql
SHOW EDGES;
```

### DESCRIBE EDGE

```sql
DESCRIBE EDGE knows;
DESC EDGE follow;
```

### DROP EDGE

```sql
DROP EDGE <edge_name>;
DROP EDGE knows;
DROP EDGE IF EXISTS knows;
```

## 온톨로지 클래스 & 일관성

클래스는 태그의 상위 호환으로, 계층(`SUBCLASS OF`)과 서로소(`DISJOINT WITH`)를
선언할 수 있습니다. 클래스를 만들면 같은 이름의 태그가 함께 생성됩니다.

```sql
CREATE CLASS <name> (<properties>)
    [SUBCLASS OF <class>[, <class> ...]]
    [DISJOINT WITH <class>[, <class> ...]];
```

```sql
CREATE CLASS animal();
CREATE CLASS dog() SUBCLASS OF animal;       -- dog ⊑ animal
CREATE CLASS building();
CREATE CLASS person() DISJOINT WITH building; -- 한 정점이 둘 다일 수 없음
```

- `SUBCLASS OF`: `MATCH ... WHERE is_a(n, "animal")` 가 dog 정점도 매칭.
- `DISJOINT WITH`: 모순(한 정점이 서로소 두 클래스에 동시 소속)을 `CHECK
  CONSISTENCY` 로 탐지.

### CHECK CONSISTENCY

선언된 disjoint 제약 위반을 보고합니다. 클래스 멤버십은 태그뿐 아니라 추론된
타입(domain/range)과 상위 클래스까지 포함하므로, 간접적으로 생긴 모순도 잡습니다.
빈 결과는 일관됨을 뜻합니다.

```sql
CHECK CONSISTENCY;
-- 결과 컬럼: vid | class_a | class_b  (위반 정점과 충돌한 disjoint 클래스 쌍)
```

## 인덱스

### CREATE INDEX

더 빠른 조회를 위해 인덱스를 생성합니다:

```sql
CREATE TAG INDEX <index_name> ON <tag_name>(<property_name>);
CREATE EDGE INDEX <index_name> ON <edge_name>(<property_name>);
```

**예시:**

```sql
CREATE TAG INDEX person_name_idx ON person(name);
CREATE EDGE INDEX follow_since_idx ON follow(since);
```

### SHOW INDEXES

```sql
SHOW TAG INDEXES;
SHOW EDGE INDEXES;
```

### DROP INDEX

```sql
DROP TAG INDEX <index_name>;
DROP EDGE INDEX <index_name>;
```

## 스키마 모범 사례

1. **적절한 데이터 타입 선택** - ID에는 `INT64`, 텍스트에는 `STRING`, 소수에는 `DOUBLE`을 사용하세요
2. **기본값을 현명하게 사용** - 합리적인 기본값을 설정하여 삽입을 단순화하세요
3. **스키마 진화를 계획** - 선택적 데이터에는 nullable 컬럼을 사용하세요
4. **인덱스 생성** - 성능을 위해 자주 쿼리하는 속성에 인덱스를 생성하세요

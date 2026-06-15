# 데이터 조작

버텍스와 엣지를 삽입, 수정, 삭제합니다.

## 버텍스

### INSERT VERTEX

```sql
INSERT VERTEX <tag_name>(<prop1>, <prop2>, ...)
VALUES <vid>:(<value1>, <value2>, ...);
```

**단일 버텍스:**

```sql
INSERT VERTEX person(name, age) VALUES 1:('Alice', 30);
INSERT VERTEX player(name, score) VALUES 100:('Bob', 2500);
```

**여러 버텍스:**

```sql
INSERT VERTEX person(name, age) VALUES
    1:('Alice', 30),
    2:('Bob', 25),
    3:('Carol', 28);
```

**기본값 사용:**

```sql
-- If player has (name STRING, score INT64 DEFAULT 0)
INSERT VERTEX player(name) VALUES 1:('NewPlayer');
-- score will be 0
```

### UPDATE VERTEX

```sql
UPDATE VERTEX ON <tag_name> <vid>
SET <property> = <value>, ...
[WHEN <condition>];
```

**예시:**

```sql
UPDATE VERTEX ON person 1 SET age = 31;
UPDATE VERTEX ON player 100 SET score = score + 100;
UPDATE VERTEX ON person 1 SET name = 'Alicia', age = 32;
```

**조건부 수정:**

```sql
UPDATE VERTEX ON player 100
SET score = score + 500
WHEN score > 1000;
```

### DELETE VERTEX

```sql
DELETE VERTEX <vid> [, <vid>, ...];
```

**예시:**

```sql
DELETE VERTEX 1;
DELETE VERTEX 1, 2, 3;
```

> **참고:** 버텍스를 삭제하면 해당 버텍스에 연결된 모든 엣지도 함께 삭제됩니다.

## 엣지

### INSERT EDGE

```sql
INSERT EDGE <edge_name>(<prop1>, <prop2>, ...)
VALUES <src_vid>-><dst_vid>:(<value1>, <value2>, ...);
```

**단일 엣지:**

```sql
INSERT EDGE knows() VALUES 1->2:();
INSERT EDGE follow(since) VALUES 1->2:(1609459200);
INSERT EDGE purchase(quantity, price) VALUES 100->200:(2, 29.99);
```

**여러 엣지:**

```sql
INSERT EDGE follow(since) VALUES
    1->2:(1609459200),
    1->3:(1612137600),
    2->3:(1614556800);
```

**랭킹 사용 (동일한 버텍스 사이의 여러 엣지):**

```sql
INSERT EDGE follow(since) VALUES 1->2@1:(1609459200);
INSERT EDGE follow(since) VALUES 1->2@2:(1612137600);
```

### UPDATE EDGE

```sql
UPDATE EDGE ON <edge_name> <src_vid>-><dst_vid>[@<rank>]
SET <property> = <value>, ...;
```

**예시:**

```sql
UPDATE EDGE ON follow 1->2 SET since = 1609459200;
UPDATE EDGE ON purchase 100->200 SET quantity = 3, price = 25.99;
```

### DELETE EDGE

```sql
DELETE EDGE <edge_name> <src_vid>-><dst_vid>[@<rank>];
```

**예시:**

```sql
DELETE EDGE knows 1->2;
DELETE EDGE follow 1->2@1;
```

## 배치 작업

대량 데이터 로딩에는 배치 삽입을 사용합니다:

```sql
INSERT VERTEX person(name, age) VALUES
    1:('User1', 20),
    2:('User2', 21),
    3:('User3', 22),
    -- ... up to 1000 vertices per batch
    1000:('User1000', 30);
```

## 트랜잭션 참고 사항

- 각 문은 원자적으로 실행됩니다
- 단일 문 내의 배치 삽입은 원자적입니다
- 문 간(cross-statement) 트랜잭션은 아직 지원되지 않습니다

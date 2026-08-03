[English](../../guide/ngql-syntax.html)

# nGQL 문법

ByoriDB는 nGQL 호환 그래프 쿼리 언어를 구현합니다. ByoriDB 확장을 포함한
집중된 부분집합이므로 다른 nGQL 구현의 모든 문법이 자동으로 지원되지는 않습니다.

## 문 종류

| 종류 | 현재 문 |
| --- | --- |
| 스페이스와 스키마 | `CREATE`, `ALTER`, `DROP`, `SHOW`, `DESCRIBE`, `USE` |
| 데이터 변경 | `INSERT VERTEX`, `UPDATE VERTEX`, `DELETE VERTEX`, `INSERT EDGE`, `DELETE EDGE` |
| 조회 | `FETCH PROP`, `GO`, `MATCH`, `LOOKUP`, `FIND`, `RECOMMEND` |
| 온톨로지 | `CREATE CLASS`, `CREATE SHAPE`, `CHECK CONSISTENCY`, `CHECK SHAPE`, `WHY` |
| 관리 | `CREATE/ALTER/DROP USER`, `GRANT/REVOKE ROLE`, `SHOW USER/USERS`, `SHOW ROLE/ROLES`, `SHOW SESSIONS`, `BALANCE` |
| 검사 | `EXPLAIN <statement>`, `PROFILE <statement>` |

실행 제약은 각 가이드 페이지를 확인하세요. 예를 들어 `UPDATE EDGE`는 parser가
받지만 동작하는 executor와 연결되지 않았고, `LOOKUP`은 현재 edge type이 아니라
tag를 대상으로 동작합니다. Secondary index fast path는 단일 field equality 조건만
사용합니다. `<`, `<=`, `>`, `>=`, `!=` 같은 조건은 지원되지만 tag scan 후
predicate를 적용하므로 range index scan으로 해석하면 안 됩니다.

## 어휘 규칙

- 키워드는 대소문자를 구분하지 않습니다. `CREATE`와 `create`는 같습니다.
- 사용자 정의 식별자는 입력 철자를 유지하며 저장된 schema와 사용자 조회에서
  대소문자를 구분합니다.
- 작은따옴표와 큰따옴표 문자열 literal을 모두 허용합니다.
- `--`는 한 줄 주석을 시작합니다.
- 마지막 세미콜론은 선택 사항입니다.

한 요청에 여러 문을 세미콜론으로 보내면 compound statement로 순서대로
실행됩니다. Transaction control 문법은 없으므로 서로 다른 요청은 하나의
transaction이 아닙니다.

```sql
CREATE SPACE demo; USE demo; SHOW TAGS;
```

compound statement는 결과를 binding해 뒤 clause에서 사용할 수 있습니다.

```sql
$friends = GO FROM 1 OVER follows YIELD dst(edge) AS vid;
FETCH PROP ON person $friends.vid;
```

지원되는 조합 방식은 이것이며 `|` pipeline operator는 구현되지 않았습니다.

## 속성 타입

현재 property schema parser는 다음 type을 허용합니다.

| 타입 | 설명 |
| --- | --- |
| `BOOL` | Boolean |
| `INT8`, `INT16`, `INT32`, `INT64` | 부호 있는 정수, `INT`는 `INT64` alias |
| `FLOAT`, `DOUBLE` | 부동소수점 수 |
| `STRING` | Text |
| `TIMESTAMP` | 정수 epoch 값 또는 허용되는 시간 문자열 |
| `DATE`, `TIME`, `DATETIME` | 지원 경로에서 허용되는 문자열이나 정수로 표현한 시간 값 |

AST에는 추가 type variant가 있지만 현재 parser는 `FIXED_STRING`과 `GEOGRAPHY`를
tag/edge property 선언으로 받지 않습니다. Parser와 실행 경로가 완성되기 전에는
해당 variant를 문서나 배포의 전제로 사용하지 마세요.

## Vertex ID

space마다 하나의 VID type을 선택합니다. `INT64`는 정수 literal을,
`FIXED_STRING(N)`은 UTF-8로 최대 `N` byte인 따옴표 문자열을 사용합니다.

```sql
CREATE SPACE demo (vid_type = INT64);
CREATE SPACE accounts (vid_type = FIXED_STRING(32));
```

문자열 VID는 standalone graph CRUD, FETCH, GO, FIND, LOOKUP, MATCH에서
지원합니다. Mapping 기반 `FIXED_STRING`은 distributed 또는 multi-coordinator
실행에서 지원하지 않으며 `RECOMMEND`는 INT64 전용입니다.
[space guide](./ngql/spaces.md#legacy-정수-bridge)의 read/delete-only live legacy
bridge를 제외하면 `FIXED_STRING` space의 정수 literal이나 `INT64` space의 문자열
literal은 거부됩니다.

## 표현식

predicate는 `==`, `!=`, `<`, `<=`, `>`, `>=`, `AND`, `OR` 등의 비교·boolean
operator를 지원합니다. `MATCH`에서는 query guide에 설명된 실행 경로를 통해 `id`,
`count`, `sum`, `avg`, `min`, `max` 등의 function과 aggregate를 사용할 수 있습니다.

mutation assignment 값은 query expression보다 제한적입니다. 특히 현재
`UPDATE VERTEX ... SET` planning은 `score = score + 1` 같은 arithmetic expression이
아니라 literal과 list 값을 처리합니다.

## 인증과 권한

statement를 실행하기 전에 인증합니다. 사용자·role 관리, `SHOW USER`,
`SHOW USERS`, `SHOW ROLE`, `SHOW ROLES`, `SHOW SESSIONS`, `BALANCE`에는 `GOD` 또는 `ADMIN`
session이 필요합니다. 나머지 statement도 인증 session의 built-in role에 따라
검사하며 compound statement의 모든 clause와 `PROFILE`이 실행하는 inner
statement까지 검사합니다.

built-in role은 현재 모든 space에 적용됩니다. Space별 grant를 표현하는 nGQL
문법이 없으므로 이 role을 tenant isolation 수단으로 사용하면 안 됩니다.

## 가이드 페이지

- [스페이스](./ngql/spaces.md)
- [스키마](./ngql/schema.md)
- [데이터 변경](./ngql/dml.md)
- [데이터 조회](./ngql/dql.md)
- [사용자와 역할](./ngql/users.md)

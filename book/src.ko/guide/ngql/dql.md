# 데이터 쿼리

[English](../../../guide/ngql/dql.html) | **한국어**

Query statement는 인증 session에서 `USE`로 선택한 space를 사용합니다. 이 문서는
현재 소스를 설명하므로 배포한 binary와 일치하는 릴리스 문서를 확인하세요.

<a id="inference-status-and-recovery"></a>

## 추론 상태와 복구

```sql
USE my_space;
CHECK INFERENCE;
REPAIR INFERENCE;
```

`CHECK INFERENCE`는 `space`, `status` column을 반환하며 상태는 `complete` 또는
`repair_required`입니다. 일반 읽기 권한으로 실행할 수 있습니다. `REPAIR INFERENCE`는
GOD/ADMIN과 쓰기 가능한 요청을 요구하고 현재 closure를 재구축한 뒤 같은 형태의 상태를
반환합니다. 반복 실행할 수 있고 asserted history를 보존합니다.

Semantic 변경은 asserted 변경과 영속 미완료 marker를 먼저 commit한 뒤 추론을 완성합니다.
실패·취소·재시작이 발생하면 복구가 성공할 때까지 graph 읽기와 추가 변경을 차단하며 상태
조회는 계속 허용합니다. 오류가 assertion rollback을 뜻하지는 않습니다. 관리자가 복구한
뒤 현재 fact를 확인하고 재시도를 결정하세요. Compound 문장은 절별 commit이며 전체
transaction이 아닙니다.

Inferred-only edge를 UPDATE하면 명시적 assertion으로 승격합니다. 직접 DELETE는
거부하므로 근거 assertion을 삭제하세요. `__inferred__` property는 엔진용으로 예약돼
있습니다. Repair는 과거 추론을 재구성하지 않으며 sameAs 병합은 계속 비가역적입니다.

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

endpoint pair로 edge를 조회하며, rank를 명시할 수 있습니다.

```sql
FETCH PROP ON knows 1->2;         // every rank of this pair
FETCH PROP ON knows 1->2@7;       // only rank 7
FETCH PROP ON knows 1->2@7, 3->4; // each reference carries its own rank
FETCH PROP ON * 1->2;
FETCH PROP ON knows "alice"->"bob";
```

**rank를 생략하면 그 pair의 모든 rank에 매치됩니다.** 결과에 `ranking` 필드가 포함되는
이유입니다. `INSERT EDGE`·`DELETE EDGE`·`UPDATE EDGE`에서 rank 생략이 rank `0`을
의미하는 것과 다릅니다 — 그 문장들은 edge 하나를 지목하지만, rank 없는 fetch는 pair에
대한 조회입니다. edge 하나만 원하면 rank를 명시하세요.

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

destination-property projection은 destination VID를 deduplicate한 뒤 내부
`batch_get` 한 번으로 읽습니다. `EXPLAIN`과 `PROFILE`에서는 이 작업을
`GetVertices`로 보여 줍니다. [이슈 #10](https://github.com/byoridb/byoridb/issues/10)의
engine 범위는 회귀 검증됐고, 외부 LDBC Q9 harness 전환과 `<10s` 수용 측정만
남았습니다.

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

### 여러 node 라벨 매칭

정점은 tag의 **집합**을 가지므로, 구분자가 여러 라벨을 어떻게 결합할지 정합니다.

```sql
MATCH (a:animal:pet) RETURN id(a) AS vid;   // every listed tag
MATCH (a:animal|pet) RETURN id(a) AS vid;   // at least one of them
```

`:`는 교집합이고 `|`는 합집합입니다. 둘 다 순서에 무관하며, 아무도 만족하지 않는
교집합은 빈 결과입니다. 라벨을 쓰지 않으면 모든 정점에 매칭됩니다.

한 패턴에서 두 구분자를 섞으면(`(a:A:B|C)`) 우선순위가 자명하지 않으므로 거부됩니다.

아래의 edge 패턴과 **의도적으로 비대칭**입니다 — 데이터 모델이 비대칭이기 때문입니다.
edge는 타입이 정확히 하나라서 `[e:a|b]`는 합집합밖에 의미할 수 없고, 정점은 tag를 여러 개
동시에 가질 수 있습니다.

### 여러 edge type 매칭

`|`로 타입을 구분합니다. 패턴은 나열한 타입 중 **아무거나** 매칭합니다 — 교집합이
아니라 합집합입니다.

```sql
MATCH (a:person)-[e:knows|follows]->(b:person) RETURN id(b) AS vid;
MATCH (a:person)-[:knows|follows|blocks*1..2]->(b:person) RETURN id(b) AS vid;
```

타입을 아예 쓰지 않으면 모든 edge type을 매칭합니다.

```sql
MATCH (a:person)-[e]->(b) RETURN type(e) AS kind;
```

콜론 반복(`[e:knows:follows]`)도 같은 의미이고 여전히 파싱됩니다 — `|`를 받기 전에는
그것이 유일하게 동작하는 표기였기 때문입니다. `|`를 쓰세요. nGQL과 Cypher가 쓰는
표기이므로 그쪽 쿼리를 그대로 옮길 수 있습니다.

comma-separated pattern은 공유 variable로 join합니다. 현재 MATCH path는
`OPTIONAL MATCH`, `GROUP BY`, `ORDER BY`, `LIMIT`, `OFFSET`도 지원합니다.

Edge variable은 저장된 edge identity를 보존합니다. `src(e)`, `dst(e)`, `type(e)`,
`rank(e)` 또는 `ranking(e)`, `properties(e)`를 사용하거나 `e` 자체를 반환할 수
있습니다. Incoming 또는 undirected pattern에서도 `src(e)`와 `dst(e)`는 저장된
방향을 유지합니다. Bound vertex를 edge accessor에 넘기면 typed bad-type null,
존재하지 않는 variable을 넘기면 typed unknown-property null을 반환합니다.

aggregate는 `COUNT`, `SUM`, `AVG`, `MIN`, `MAX`를 지원합니다.

```sql
MATCH (p:person)
RETURN p.person.city AS city, COUNT(*) AS people
GROUP BY p.person.city
ORDER BY people DESC
LIMIT 10;
```

**현재 MATCH 계약:** projection·
WHERE·grouping·aggregate 평가에서 저장소·decode·VID 해석 오류를 없는 값으로 바꾸지
않고 전파합니다. 실제로 없는 속성과 typed null은 기존 의미를 유지하고 직접 지정한 VID가
없으면 매칭 없음입니다. Decode한 MATCH 후보·방향별/optional edge·중간 vertex의
identity를 검사합니다. Required/OPTIONAL의 bound 시작점은 label과 literal property를
다시 확인합니다. 무방향 self-loop는 전체 edge identity로 한 번만 세고 다른 rank·반대
방향 edge는 구분합니다. Required/OPTIONAL의 bound 도착점을 덮어쓰지 않으며 반복
node 변수는 같은 VID, 반복 edge 변수는 전체 edge identity를 유지합니다. node/edge binding 종류 충돌을
만나면 명시적 오류입니다. OPTIONAL이 새로 도입한 null binding을
뒤에서 다시 바인딩하지 않습니다. 실제 decode한 레코드의 검증이며 전체 무결성 검사는 아닙니다.

`COUNT(expr)`은 null을 제외하고 `MIN/MAX`도 null을 건너뜁니다. 빈 입력의 global
aggregate는 `LIMIT/OFFSET` 적용 전 한 행을 만듭니다. 결과 범위는 집계 후 적용하며
집계 overflow 검사를 우회하지 않습니다. 정수만의 `SUM`은 정확한 정수를 반환하거나
범위 초과 오류가 됩니다. 실수 `SUM/AVG`는 비유한 결과를 거부하지만 혼합 타입·비유한
`MIN/MAX` 입력의 완전한 정렬 순서를 정의한다는 뜻은 아닙니다.

Grouping은 타입을 보존하는 정규화 key를 사용합니다. Map 항목 순서는 무시하고
vertex/edge는 graph identity를 사용하며 실수 `+0`/`-0`은 같은 group으로 묶습니다.
정수와 실수는 다른 group입니다. 비유한 값과 Path/Set/DataSet/Geography group key는
명시적으로 미지원입니다. `ORDER BY`는 모호하지 않은 출력 별칭 또는 projected 표현식과
정확히 일치해야 하며 없거나 모호한 참조는 scan 전에 거부합니다. 집계 출력 행은 공통
추정 결과 메모리 예산을 검사하지만 grouping 중간 메모리 전체를 제한하지는 않습니다.

Planner는 MATCH의 미지원 산술·단항·map 생성 표현식, `COLLECT`, 중첩·잘못 배치한
집계, 비literal node/edge pattern property를 빈 입력에서도 데이터 접근 전에 거부합니다.
집계는 top-level RETURN 호출만 지원하므로 scalar 함수·list 안에 중첩하거나 WHERE/
GROUP BY 표현식으로 사용할 수 없습니다. `properties()`가 반환하는 property map을
금지하는 것은 아닙니다. 현재 실행 경로는 `isa`를 `is_a`의 별칭으로도 받습니다.
역방향 조회 shortcut은 range 없는 outgoing pattern으로 제한하며 `EXPLAIN`도 같은
방향/range 제한을 적용합니다. MATCH 의미 전체를 완성했다는 주장은 아니며 QRY-2는
IN PROGRESS입니다.

ontology-aware membership는 `is_a`를 사용합니다.

```sql
MATCH (n:dog) WHERE is_a(n, "animal") RETURN id(n) AS vid;
```

## 함수

엔진이 구현하지 않은 함수를 호출하면 `NULL`이 아니라 **함수 이름을 명시한 query
오류**가 됩니다. `WHERE`에서 특히 중요합니다 — null 술어는 모든 행에 대해 false이므로,
미지원 함수가 결과 0건을 보고하면 "아무것도 매치되지 않음"과 구분할 수 없습니다.

```
nebula> MATCH (n:doc) WHERE frobnicate(n.doc.body) RETURN n;
[ERROR] Unknown function: frobnicate
```

### 그래프 함수

`MATCH` 안에서 인자 값이 아니라 그래프 상태를 읽습니다.

| 함수 | 반환 |
| --- | --- |
| `id(v)` | bound vertex의 VID |
| `src(e)`, `dst(e)` | 저장된 방향 기준 edge 양 끝점 |
| `type(e)` | edge type 이름 |
| `rank(e)`, `ranking(e)` | edge rank |
| `properties(v)`, `properties(e)` | 모든 property의 flat map |
| `tags(v)`, `labels(v)` | vertex의 tag 이름들 |
| `is_a(v, "Class")` | `SUBCLASS OF` 상위 클래스를 포함한 클래스 소속 여부 |

### 집계

지원하는 MATCH 집계는 `COUNT`, `SUM`, `AVG`, `MIN`, `MAX`입니다.
`COLLECT`는 미구현이며 현재 실행 경로는 데이터 접근 전에 거부합니다. 현재 동작에서
`COUNT(*)`는 행을 세고 `COUNT(expr)`은 null 값을 제외합니다. 지원되는 단순 단일 node
`COUNT(*)`는 검증한 후보를 streaming 집계하며 grouped/edge count는 일반 matching을
사용합니다. 두 경로 모두 해당 scan 예산과 결과 범위 의미를 유지합니다.

### 스칼라 함수

표현식이 평가되는 모든 위치에서 사용할 수 있습니다.

| 분류 | 함수 |
| --- | --- |
| 대소문자 | `lower` / `toLower`, `upper` / `toUpper` |
| 크기 | `length` / `size` |
| 텍스트 | `contains`, `starts_with` / `startsWith`, `ends_with` / `endsWith` |
| 수치 | `abs`, `floor`, `ceil`, `round` |
| Null | `is_null` / `isNull`, `is_not_null` / `isNotNull`, `coalesce` |

함수 이름은 대소문자를 구분하지 않으므로 `toLower`, `TOLOWER`, `tolower`는 같은
함수입니다. 지원되는 함수에 잘못된 타입의 인자를 주면 query가 실패하는 대신 typed
null이 되고, **이름을 모르는 경우에만** 거부됩니다.

### 텍스트 매칭은 대소문자를 구분합니다

`CONTAINS`, `STARTS WITH`, `ENDS WITH`는 **정확히** 비교하므로
`CONTAINS 'worktrees'`는 `Worktrees`에 매치되지 않습니다. 철자별로 여러 query를
보내는 대신 양쪽을 접으세요.

```sql
MATCH (n:doc) WHERE toLower(n.doc.body) CONTAINS 'worktrees'
RETURN n.doc.body AS body;
```

대소문자를 무시하는 비교 연산자는 없습니다. `toLower`로 접는 것이 지원되는
방법입니다.

## LOOKUP

`LOOKUP`은 tag와 edge type을 모두 대상으로 합니다. 이름만 쓰면 schema에 조회해
어느 쪽인지 결정합니다.

```sql
LOOKUP ON person WHERE person.name == "Alice";
LOOKUP ON person WHERE person.age >= 21 YIELD person.name, person.age LIMIT 20;

LOOKUP ON knows WHERE knows.since == 2020;
LOOKUP ON knows WHERE since == 2020 LIMIT 20;
```

한 space에 **같은 이름**의 tag와 edge type이 함께 있을 수 있고, 이름만 쓰면 그런
이름은 **tag**로 해석됩니다. edge를 지목하려면 `LOOKUP ON EDGE`를 쓰세요 — 그러지
않으면 그 edge에 도달할 방법이 없고, edge 속성에 대한 조건이 tag에 걸려 아무것도
반환하지 않습니다.

```sql
CREATE TAG dup(x INT64);
CREATE EDGE dup(y INT64);

LOOKUP ON dup WHERE dup.x == 7;        // the tag
LOOKUP ON EDGE dup WHERE dup.y == 9;   // the edge
```

`LOOKUP ON TAG`는 없습니다. 이름만 쓰는 형태가 이미 tag를 우선하며, 명시적으로 썼는데도
해당 tag가 없을 때 edge로 fallback하면 오해를 부르기 때문입니다.

tag `LOOKUP`은 `<tag>.vid`와 tag 속성을 반환합니다. edge `LOOKUP`은
**`<edge>.src`·`<edge>.dst`·`<edge>.rank`** 를 반환합니다 — rank가 edge identity의
일부이므로 이 셋이 함께 edge 하나를 지목합니다. 속성은 tag와 마찬가지로 이름 그대로와
수식한 형태 둘 다로 매칭됩니다(`since`와 `knows.since`).

indexed 속성의 동등 조건은 tag와 edge type 모두 secondary index를 사용할 수 있습니다.
Standalone mode에서 지원되는 ordered range는 single-field index를 사용하며, 그 밖의
지원되는 조건은 bounded predicate scan을 사용합니다.

- **edge type** 순서 비교(`>`, `>=`, `<`, `<=`)는 property schema와 경계 타입이
  호환될 때 index를 사용합니다. Float/Double이면 정수·실수 key 영역을 모두 포함하도록
  predicate scan을 사용합니다. `EXPLAIN`도 같은 선택을 표시합니다. 문자열 경계는 length-prefixed index가
  사전순을 보장하지 않으므로 scan으로 fallback합니다.
- **tag**의 순서 비교 조건은 Boolean 또는 정수 literal이 schema type과 일치할 때
  single-field index를 사용합니다. Float/Double property는 정수와 실수가 서로 다른
  index key 영역에 놓이므로 scan으로 fallback합니다.
- Distributed 실행에서는 ordered `LOOKUP` range를 거부합니다.
- pushdown이 표현할 수 없는 조건(`CONTAINS`, `STARTS WITH`, 필드 간 비교)은 조용히 전체를
  매칭하지 않고 **거부**됩니다.

기본 scan 예산은 후보 100,000개이며 `BYORIDB_MAX_SCAN_LIMIT`으로 설정합니다.

**현재 소스의 계약:** 로컬 tag/edge LOOKUP은 조건 필터와
낡은 index 항목 재검증 전에 full-prefix 또는 index partition scan별 원시 후보를 셉니다.
`0`은 해당 예산을 끕니다. 정확히 cap이면 성공할 수 있고 cap+1번째 후보를 발견하면
잘린 성공 대신 오류를 반환합니다. Query `LIMIT/OFFSET`은 결과 범위를 별도로 정하므로
`LIMIT 1`도 후보 예산 초과를 숨기지 못합니다. 이는 scan별 예산이며 전체 작업량·edge
개수·메모리의 전역 상한이 아닙니다. 관련 index partition·schema·decode 실패를 전파하고
손상된 edge-index key를 거부합니다. LOOKUP/FIND의 공통 edge decoder는 잘못된 JSON,
구조 멤버 타입, 명시된 key/payload identity 불일치를 거부하며 LOOKUP은 vertex도
검사합니다. 이전 형식에서 생략한 vertex VID·edge rank·properties의 기본값은 유지합니다.
실제 decode한 레코드가 대상이며 LIMIT이 이후 decode를 멈추거나 FIND가 무관한 edge
type을 건너뛸 수 있습니다. 수집한 모든 후보의 검사·모든 손상 탐지·FIND의 vertex 레코드
검증을 뜻하지 않습니다.

분산 indexed LOOKUP은 전체 partition topology를 검증하고 partition마다 같은 원시
후보 예산과 query에 묶인 keyset cursor를 적용합니다. 소유 storage service에서 payload를
읽고 identity와 타입 있는 property를 검증하며 누락되거나 실패한 partition이 있으면
전체 조회를 실패시킵니다. 결과 LIMIT으로 partition별 예산을 나누지 않습니다. Page는
동시 쓰기에 대한 snapshot을 제공하지 않으며 분산 full scan과 ordered range는 계속
미지원입니다. 이 component 보장이 standalone launcher의 복제 cluster 실행을 뜻하지는
않습니다.

Float 동등 비교는 기존 epsilon 비교를 사용하므로 정확한 float key 하나만 읽지 않고
선택한 index 전체를 읽어 재검사합니다. 모든 후보가 scan 예산에 포함됩니다. Integer와
float의 동등 비교는 타입을 구분하므로 수치가 같아도 정수 literal이 저장된 실수와
일치하지는 않습니다.

index 후보는 항상 저장된 vertex·edge와 대조해 재검증하므로, 낡은 index 항목이 결과가
될 수 없습니다. 실제 access path는 `EXPLAIN` 또는 `PROFILE`로 확인하세요.

```
EXPLAIN LOOKUP ON knows WHERE knows.since == 2020;
// IndexScan  on Edge knows where knows.since == 2020   index: knows_since_idx

EXPLAIN LOOKUP ON knows WHERE knows.since > 2019;
// IndexScan  on Edge knows where knows.since > 2019    index: knows_since_idx

EXPLAIN LOOKUP ON tagged WHERE tagged.label > 'a';
// EdgeScan   on Edge tagged where tagged.label > "a"   ⚠ FULL SCAN
```

## FIND path

```sql
FIND SHORTEST PATH FROM 1 TO 3 OVER knows;
FIND SHORTEST PATH FROM 1 TO 3 OVER road WEIGHT BY distance;
FIND SHORTEST PATH FROM 1 TO 3 OVER knows BIDIRECT UPTO 5 STEPS;
FIND ALL SHORTEST PATHS FROM 1 TO 3 OVER knows UPTO 5 STEPS;
```

`OVER`는 하나의 edge type 또는 `*`를 받으며 comma-separated edge list는 받지 않습니다.
`FIND WHERE`와 `FIND YIELD`는 미구현이며 vertex ID 해석 전에 해당 절을
명시적으로 거부합니다.

**현재 소스의 계약:**

- `max_traversal_nodes`는 확장한 vertex를 세며 hop 제한이 있는 가중치 조회는
  vertex/hop 상태를 셉니다. 기본 100,000이며 `0`이면 비활성화합니다. 초과하면 조회가
  실패합니다. 대기 중인 frontier 메모리나 읽은 edge 개수의 상한은 아닙니다.
- `FIND ALL SHORTEST PATHS`의 `max_find_paths`는 기본 1,024이며 `0`이면 비활성화합니다.
  정확히 그 개수의 완성 경로는 성공할 수 있고 경로 하나를 더 찾으면 출력을 자르지 않고
  실패합니다.
- 명시한 `UPTO n STEPS`는 가중치·비가중치 경로 모두의 hop을 제한합니다. 생략하면
  비가중치는 기본 10단계, 가중치는 hop 제한이 없으며 실행 작업 예산은 계속 적용됩니다.
  `u32::MAX`를 넘는 `UPTO`는 wrap하지 않고 파싱 단계에서 거부합니다.
- `WEIGHT BY` 속성이 없거나 숫자가 아니면 기존 단위 가중치 기본값 `1`을 유지합니다.
  숫자 가중치는 음수가 아닌 유한 값이어야 합니다. 도달 가능한 가중치의 보수적 검증은
  반환 경로 외의 작업을 추가할 수 있으며 음수·비유한 숫자나 관련 edge 데이터 손상은
  오류가 됩니다. 저비용 목표 경로를 일찍 찾았다고 검증·실행이 끝난 것은 아닙니다.

현재 checkout의 계약이며 이전 릴리스의 동작은 바뀌지 않습니다. 상한과 남은 실행 제약은
[Runtime guard](../../reference/performance.md#runtime-guard)를 참고하세요.

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

`RECOMMEND`는 현재 정수 seed를 받고 정수 VID를 반환하므로 `INT64` space에서만
지원합니다. `FIXED_STRING` space에서는 실행하지 마세요.

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

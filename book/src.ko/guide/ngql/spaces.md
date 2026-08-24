[English](../../../guide/ngql/spaces.html)

# 스페이스

space는 graph schema와 data의 logical namespace입니다. Tag나 edge를 만들거나
graph record를 읽고 쓰기 전에 `USE`로 space를 선택합니다.

## 스페이스 생성

space는 정수 VID 또는 고정 길이 문자열 VID를 사용할 수 있습니다.

```sql
CREATE SPACE social;
CREATE SPACE IF NOT EXISTS social (vid_type = INT64);
CREATE SPACE accounts (vid_type = FIXED_STRING(32));
```

현재 기본값은 `partition_num = 100`, `replica_factor = 1`,
`vid_type = INT64`, hash partition metadata입니다. Option을 명시할 수도 있습니다.

```sql
CREATE SPACE analytics (
    partition_num = 16,
    replica_factor = 1,
    vid_type = INT64
) PARTITION BY HASH;
```

parser는 `PARTITION BY MODULO`와 `PARTITION BY RANGE(100, 200, 300)`도
받습니다. Standalone mode에서는 이 값이 metadata로 저장될 뿐 한 process를
multi-node cluster로 바꾸지 않습니다.

`FIXED_STRING(N)`은 UTF-8로 인코딩했을 때 최대 `N` byte인 따옴표 문자열 VID를
받습니다. 한 space에서는 VID type을 일관되게 사용하므로 보통 `FIXED_STRING`
space의 정수 literal과 `INT64` space의 문자열 literal은 명확한 오류로 거부됩니다.

```sql
USE accounts;
CREATE TAG account(name STRING);
INSERT VERTEX account(name) VALUES "acct-001":("Primary");
FETCH PROP ON account "acct-001";
```

내부적으로 `FIXED_STRING` space는 UTF-8 VID와 안정적인 **음수** `i64`
surrogate의 양방향 mapping을 영속 저장합니다. 새 mapping은 0 이상의 값을 절대
사용하지 않습니다. 문자열→surrogate record key는
`{space}:vid-map:{hex-utf8}`이고 value는 signed `i64`를 정확히 8-byte big-endian으로
인코딩합니다. 역방향 key는 `{space}:vid-rev:{surrogate}`이고 value는 원래 UTF-8
byte입니다. Graph data를 지워도 mapping은 재사용하지 않으며 `DROP SPACE`의 일반
space prefix 삭제 때 함께 제거됩니다.

이 mapping은 [이슈 #49](https://github.com/byoridb/byoridb/issues/49)의 정수 계약을
보존합니다. Vertex, edge, tag-to-VID, index, partition, codec, storage key는 계속
`i64`를 사용하고 storage RPC/protobuf message도 기존 정수 VID field를 유지합니다.
Query와 API 입력·결과만 executor 경계에서 변환합니다.

Mapping 기반 `FIXED_STRING` 실행은 standalone mode에서만 지원합니다. Mapping
record에 cluster-wide ownership, replication, consensus, routing이 아직 없으므로
distributed 또는 multi-coordinator 배포는 `INT64` space를 사용해야 합니다.
`RECOMMEND`도 현재 INT64 전용입니다. [데이터 쿼리](./dql.md#recommend)를 참고하세요.

### Legacy 정수 bridge

영속 문자열 mapping 도입 전에 만든 space에는 0 이상의 정수 VID로 저장된 live graph
record가 있을 수 있습니다. ByoriDB는 **실제로 존재하며 mapping되지 않은 0 이상의**
VID만 임시 read/delete compatibility bridge로 노출합니다. 결과는 정수이고 operator
warning을 남깁니다. 해당 legacy ID는 write-frozen 상태입니다. 정수 endpoint를 사용한
INSERT, UPDATE, 새 edge write는 거부하며 raw 음수 internal surrogate도 항상 거부합니다.

이 bridge는 **정상 운영 상태가 아니라 전환 장치**입니다. 업그레이드된 space를
마이그레이션할 수 있을 만큼만 읽히게 유지하는 것이 목적이고, 사용되면 space당 한 번
warning을 남깁니다. 두 namespace를 섞어 쓰는 것은 지원되는 운영 mode가 아닙니다. Live
vertex나 incident-edge 근거가 없는 unmapped 정수는 일반 point miss입니다.

### 클라이언트가 해싱한 정수 VID에서 마이그레이션

`FIXED_STRING` space가 없던 시절에는 이름으로 entity를 키잉하는 클라이언트가 이름을
직접 `i64`로 해싱해야 했습니다. 그런 space는 평범한 `INT64` space이므로 **위의 legacy
bridge가 적용되지 않습니다** — bridge는 descriptor가 이미 `FIXED_STRING`인 space를
위한 것입니다.

**space의 VID 타입은 생성 시점에 고정됩니다.** `ALTER`에는 `TAG`·`EDGE`·`USER` 형태만
있고 `ALTER SPACE`는 없으며, `vid_type`은 `CREATE SPACE`가 기록한 space descriptor에서
읽습니다. 따라서 마이그레이션은 in-place 변환이 아니라 **새 space로의 복사**입니다.

```sql
-- 1. 대상 space 생성. N은 가장 긴 식별자를 바이트 단위로 담을 수 있어야 합니다.
CREATE SPACE memory_v2 (vid_type = FIXED_STRING(128));
USE memory_v2;

-- 2. schema를 다시 만듭니다. tag·edge·index·class·shape는 데이터와 함께
--    옮겨오지 않습니다.
CREATE TAG note(name STRING, body STRING);
CREATE EDGE rel(kind STRING);

-- 3. 이전 space에서 vertex와 edge를 읽어, 클라이언트가 해싱하던 그 이름을
--    문자열 식별자로 써서 삽입합니다.
INSERT VERTEX note(name, body) VALUES "decision:use-redb":("decision:use-redb", "adopt redb");
INSERT EDGE rel(kind) VALUES "decision:use-redb"->"module:kvstore":("affects");
```

이후 조회는 문자열을 반환하고 traversal도 문자열에서 출발합니다.

```sql
MATCH (n:note) WHERE id(n) == "decision:use-redb" RETURN id(n) AS vid;
GO FROM "decision:use-redb" OVER rel YIELD rel.kind AS kind;
```

새 space에 정수 VID를 쓰는 것은 거부됩니다 — 같은 entity에 두 번째 신원이 조용히
생기지 않습니다.

```
INSERT VERTEX note(name, body) VALUES 111:("x", "y");
[ERROR] space 'memory_v2' uses FIXED_STRING VIDs; integer VID 111 is
        read/delete-only legacy data and cannot be written
```

internal surrogate도 지정할 수 없습니다. 쓰기에서는 위의 정수 규칙이 음수를 포함한 모든
정수에 적용되고, 읽기·삭제에서는 raw 음수 VID가 internal surrogate로서 거부됩니다.

#### 이력은 마이그레이션을 따라오지 않습니다

이것이 계획에 반영해야 할 결과입니다. 이력은 space와 VID로 키잉되므로, 새 식별자로 다시
삽입하면 그 삽입 시점에서 시작하는 **새 이력**이 생깁니다.

```sql
-- 새 space에서 삽입 이전 시점: 0건.
FETCH PROP ON note "decision:use-redb" AS OF 1785283200000;

-- 이전 space에는 마이그레이션 전 상태가 이전 VID 아래 그대로 남아 있습니다.
USE memory_v1;
FETCH PROP ON note 111 AS OF 1785283200000;
```

즉 마이그레이션은 rename이 아니라 **현재 사실의 재단정**입니다. 과거 상태가 필요하면
이전 space를 기록 보관소로 유지하고, 새 space에서 `AS OF`가 계속 동작하리라 기대하며
`DROP`하지 마세요. 재키를 넘어 이력을 옮기는 방법은 지원되지 않습니다.

#### 마이그레이션 전에 확인할 것

- `RECOMMEND`는 INT64 전용이며 `FIXED_STRING` space에서는 space 이름을 명시해 거부됩니다.
  이 기능에 의존하는 workload라면 해당 데이터는 `INT64` space에 두세요
  ([데이터 조회](./dql.md#recommend) 참고).
- mapping 기반 `FIXED_STRING` 실행은 위와 같이 standalone 전용입니다.
- 식별자 길이 제한은 문자 수가 아니라 `N` **바이트**이므로 비ASCII 이름은 여유가
  필요합니다.

## 스페이스 선택

```sql
USE social;
```

선택한 space는 인증 session에 속합니다. 같은 session의 이후 요청은 다른 `USE`가
성공할 때까지 그 space를 사용합니다.

## 스페이스 조회

```sql
SHOW SPACES;
DESCRIBE SPACE social;
```

`SHOW SPACES`는 저장된 각 space의 ID와 configuration metadata를 보여 줍니다.

## 스페이스 삭제

```sql
DROP SPACE social;
DROP SPACE IF EXISTS social;
```

`DROP SPACE`는 파괴적 작업입니다. 해당 space의 schema, current graph data,
관련 index data를 제거합니다. 현재 prefix deletion 경로는 별도 temporal-history
table을 지우지 않으므로 `DROP SPACE`는 모든 과거 byte의 안전한 삭제가 아닙니다.
Session에서 space를 나가는 용도로 쓰지 말고 `USE`로 다른 space를 선택하세요.

## 권한

- `USE`, `SHOW`, `DESCRIBE`에는 read access가 필요합니다.
- `CREATE SPACE`에는 새 space 이름에 대한 create access가 필요합니다.
- `DROP SPACE`에는 drop access가 필요합니다.

모든 built-in role permission은 현재 wildcard space `*`를 사용합니다. 사용자용
space-scoped `GRANT` 문법이 없으므로 built-in role은 모든 space에 적용됩니다.

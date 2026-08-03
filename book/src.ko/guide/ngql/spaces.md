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

이 bridge를 mixed-VID 운영 mode로 사용하지 마세요. Legacy graph를 export하고
`FIXED_STRING` space를 다시 만든 뒤 따옴표 문자열 VID로 import해야 합니다. Live
vertex나 incident-edge 근거가 없는 unmapped 정수는 일반 point miss입니다.

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

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
받습니다. 한 space에서는 VID type을 일관되게 사용하므로 `FIXED_STRING` space의
정수 literal과 `INT64` space의 문자열 literal은 명확한 오류로 거부됩니다.

```sql
USE accounts;
CREATE TAG account(name STRING);
INSERT VERTEX account(name) VALUES "acct-001":("Primary");
FETCH PROP ON account "acct-001";
```

내부적으로 `FIXED_STRING` space는 UTF-8 VID와 안정적인 양수 `i64` surrogate의
양방향 mapping을 영속 저장합니다. 기존 vertex, edge, tag-to-VID, index,
partition, codec, storage, RPC 형식은 이 surrogate를 계속 사용하고, graph query
입력과 결과는 executor 경계에서 변환합니다. Mapping key는 문자열에서 surrogate
방향이 `{space}:vid-map:{hex-utf8}`, 역방향이
`{space}:vid-rev:{surrogate}`입니다. Graph data를 지워도 mapping은 재사용하지
않으며 `DROP SPACE`의 일반 space prefix 삭제 때 함께 제거됩니다. 따라서 기존
정수 storage와 protobuf wire contract를 유지하면서 client에는 원래 문자열을
돌려줍니다.

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

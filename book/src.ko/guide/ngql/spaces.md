[English](../../../guide/ngql/spaces.html)

# 스페이스

스페이스는 그래프 스키마와 데이터의 논리적 네임스페이스입니다. tag나 edge를
만들거나 그래프 레코드를 읽고 쓰기 전에 `USE`로 스페이스를 선택합니다.

## 스페이스 생성

안정적인 스탠드얼론 형식은 정수 VID를 사용합니다.

```sql
CREATE SPACE social;
CREATE SPACE IF NOT EXISTS social (vid_type = INT64);
```

현재 기본값은 `partition_num = 100`, `replica_factor = 1`,
`vid_type = INT64`, hash 파티션 메타데이터입니다. 옵션을 명시할 수도 있습니다.

```sql
CREATE SPACE analytics (
    partition_num = 16,
    replica_factor = 1,
    vid_type = INT64
) PARTITION BY HASH;
```

파서는 `PARTITION BY MODULO`와 `PARTITION BY RANGE(100, 200, 300)`도
받습니다. 스탠드얼론 모드에서는 이 값이 메타데이터로 저장될 뿐 한 프로세스를
다중 노드 클러스터로 바꾸지 않습니다.

`FIXED_STRING(N)`도 스페이스 VID 타입 옵션으로 받지만 현재 DML 계획은 정수
리터럴 VID를 요구합니다. 문자열 VID 실행이 구현될 때까지 `INT64`를 사용하세요.

## 스페이스 선택

```sql
USE social;
```

선택한 스페이스는 인증 세션에 속합니다. 같은 세션의 이후 요청은 다른 `USE`가
성공할 때까지 그 스페이스를 사용합니다.

## 스페이스 조회

```sql
SHOW SPACES;
DESCRIBE SPACE social;
```

`SHOW SPACES`는 저장된 각 스페이스의 ID와 설정 메타데이터를 보여 줍니다.

## 스페이스 삭제

```sql
DROP SPACE social;
DROP SPACE IF EXISTS social;
```

`DROP SPACE`는 파괴적 작업입니다. 해당 스페이스의 스키마, 현재 그래프 데이터,
관련 인덱스 데이터를 제거합니다. 현재 prefix 삭제 경로는 별도 temporal-history
테이블을 지우지 않으므로 `DROP SPACE`는 모든 과거 byte의 안전한 삭제가 아닙니다.
세션에서 스페이스를 나가는 용도로 쓰지 말고 `USE`로 다른 스페이스를 선택하세요.

## 권한

- `USE`, `SHOW`, `DESCRIBE`에는 읽기 권한이 필요합니다.
- `CREATE SPACE`에는 새 스페이스 이름에 대한 생성 권한이 필요합니다.
- `DROP SPACE`에는 drop 권한이 필요합니다.

모든 내장 역할 권한은 현재 wildcard 스페이스 `*`를 사용합니다. 사용자용
스페이스별 `GRANT` 문법이 없으므로 내장 역할은 모든 스페이스에 적용됩니다.

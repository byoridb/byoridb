# 사용자와 역할

[English](../../../guide/ngql/users.html)

ByoriDB는 인증된 session에 역할 기반 권한 검사를 적용합니다. 현재 역할은 모든 space에
걸쳐 적용되며 space별 tenant 격리를 제공하지 않습니다.

## Root 초기 설정

Standalone server는 시작 전에 비어 있지 않은 `BYORIDB_ROOT_PASSWORD`를 요구합니다.

```bash
export BYORIDB_ROOT_PASSWORD='value-from-your-secret-manager'
byoridb-server
```

비밀번호는 로그에 출력하지 않습니다. `root`는 예약된 사용자 이름이며 nGQL로 생성,
삭제하거나 변경할 수 없습니다. `BYORIDB_ROOT_PASSWORD`를 바꾸고 서버를 재시작하여
root credential을 교체하세요.

## 관리 권한

다음 명령은 `GOD` 또는 `ADMIN` session에서만 실행할 수 있습니다.

- `CREATE USER`, `ALTER USER`, `DROP USER`
- `GRANT ROLE`, `REVOKE ROLE`
- `SHOW USERS`, `SHOW ROLES`, `SHOW SESSIONS`
- `BALANCE`

세미콜론으로 구분한 compound request나 실행하는 `PROFILE` 내부의 명령에도 같은
검사를 적용합니다. 현재 인터페이스에서는 관리자가 아닌 사용자가 `ALTER USER`로 자신의
비밀번호를 바꿀 수도 없습니다.

## 사용자 생성

```sql
CREATE USER alice WITH PASSWORD "a-long-random-password";
CREATE USER bob WITH PASSWORD "another-long-password" ROLE USER;
CREATE USER IF NOT EXISTS report_reader WITH PASSWORD "reader-password" ROLE GUEST;
```

현재 사용자 관리 경로는 비밀번호의 최소 길이나 강도 정책을 강제하지 않습니다. 입력된
값은 salt가 적용된 Argon2 hash로 저장되지만 운영자는 고유하고 entropy가 충분한 값을
요구해야 합니다. `ROLE`을 생략한 사용자는 role을 부여할 때까지 권한이 없습니다.

사용자 이름은 대소문자를 구분합니다. 예약된 root 이름은 대소문자를 구분하지 않습니다.

현재 Graph query log와 active-query diagnostics는 raw query text를 제외합니다.
CLI는 입력한 문장을 로컬 `history.txt`에 저장하므로 사용자 관리 문장의 비밀번호가
남을 수 있습니다. 작업 후 해당 파일을 보호하거나 삭제하세요.

## 사용자 변경 및 삭제

```sql
ALTER USER alice WITH PASSWORD "a-new-long-password";
DROP USER alice;
DROP USER IF EXISTS alice;
```

현재 단일 프로세스 graph service에서는 비밀번호, role, 활성 상태 변경 및 사용자
삭제가 해당 사용자의 session을 폐기합니다. 이후 요청은 다시 인증해야 합니다. 미완성
distributed mode에서 cluster 전체의 session 폐기는 아직 운영상 보장되지 않습니다.

## 기본 역할

| 역할 | 실제 권한 |
| --- | --- |
| `GOD` | 모든 space에서 read, write, create, alter, drop 및 관리 명령 |
| `ADMIN` | 현재 `GOD`와 같은 권한 및 관리 명령 |
| `DBA` | 모든 space에서 read, write, create, alter; drop과 사용자 관리 제외 |
| `USER` | 모든 space에서 read 및 graph data write |
| `GUEST` | 모든 space에서 read-only |

Graph data write에는 INSERT, UPDATE, DELETE가 포함됩니다. `ALTER` 문장은 현재
DBA가 가진 `Create` 권한으로 검사합니다. ADMIN은 사용자 관리와 특권 role 부여가
가능하므로 전적으로 신뢰하는 역할로 취급해야 합니다.

모든 기본 permission entry는 현재 wildcard space `*`를 사용합니다.
`GRANT ... ON <space>` 문법은 없으므로 같은 기본 role을 가진 사용자 사이의 보안
경계로 space를 분리해서는 안 됩니다.

## 역할 부여 및 회수

```sql
GRANT ROLE USER TO alice;
GRANT ROLE DBA TO database_operator;
REVOKE ROLE USER FROM alice;
```

Role 이름은 대문자로 정규화합니다. 영속 사용자에게는 `ADMIN`, `DBA`, `USER`,
`GUEST`를 부여할 수 있으며 `GOD`는 프로세스가 관리하는 root identity 전용입니다.
Role을 부여하거나 회수하면 local service에서 해당 사용자의 현재 session을 폐기합니다.

## 사용자·역할·세션 조회

```sql
SHOW USERS;
SHOW ROLES;
SHOW SESSIONS;
```

`SHOW USERS`는 built-in root와 영속 사용자를 password hash 없이 `Name`, `Role`로
반환합니다. `SHOW ROLES`는 역할 정의 목록이 아니라 같은 사용자/역할 목록의
alias입니다. Singular `SHOW USER`와 `SHOW ROLE`도 호환 alias로 허용합니다.

`SHOW SESSIONS`는 graph service에서 구현되어 있으며 사용자와 선택된 space만
반환합니다. Bearer session ID는 노출하지 않습니다. Session은 무작위 양의 63-bit
식별자를 사용하며 기본 sliding idle 수명은 24시간입니다. `auth.session_ttl_secs`로
변경할 수 있습니다.

Graph service 없이 executor를 직접 임베드한 경로에는 session 원본이 없으므로
`SHOW SESSIONS`는 빈 목록 대신 명시적 unsupported 오류를 반환합니다.

모든 session ID를 bearer credential로 취급하세요. 로그에 넣거나 다른 사용자에게
노출하지 마세요.

## 배포 보안

Native gRPC와 HTTP listener는 TLS를 종료하지 않습니다. ByoriDB는 process-local
로그인 실패 제한, 계정 lockout 및 동시 실행 수가 제한된 Argon2 검증을 제공하지만
일반 connection이나 query traffic을 제한하지는 않습니다. 비로컬 배포에서는 신뢰할 수
있는 TLS endpoint, 방화벽 또는 network policy, edge rate limit을 적용하세요. Root와
사용자 credential은 저장소 파일이나 command-line 인자 대신 secret manager에 보관하세요.

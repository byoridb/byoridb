# 사용자 관리

ByoriDB는 사용자와 권한을 관리하기 위한 역할 기반 접근 제어(RBAC)를 제공합니다.
사용자 생성·변경·삭제와 역할 부여/회수는 GOD 또는 ADMIN 세션에서만 실행할 수 있습니다.

## 사용자

### CREATE USER

비밀번호와 선택적 역할을 지정하여 새 사용자를 생성합니다:

```sql
CREATE USER <username> WITH PASSWORD '<password>' [ROLE <role>];
```

**예시:**

```sql
-- Create user without specifying a role (no roles assigned by default)
CREATE USER alice WITH PASSWORD 'secure123';

-- Create user with specific role
CREATE USER bob WITH PASSWORD 'pass456' ROLE ADMIN;

-- Create user if not exists
CREATE USER IF NOT EXISTS charlie WITH PASSWORD 'mypass';
```

### ALTER USER

사용자의 비밀번호를 변경합니다:

```sql
ALTER USER <username> WITH PASSWORD '<new_password>';
```

**예시:**

```sql
ALTER USER alice WITH PASSWORD 'newsecure456';
```

### DROP USER

사용자를 삭제합니다:

```sql
DROP USER <username>;
DROP USER IF EXISTS <username>;
```

**예시:**

```sql
DROP USER alice;
DROP USER IF EXISTS bob;
```

> **참고:** `root` 사용자는 삭제할 수 없습니다.

### SHOW USERS

GOD 또는 ADMIN 사용자는 built-in `root`와 KVStore에 영속된 사용자를 조회할 수 있습니다.
결과는 사용자 이름순이며, 여러 역할은 쉼표로 구분됩니다. 역할이 없는 사용자는 Role
열이 빈 문자열입니다.

```sql
SHOW USERS;
```

## 역할

ByoriDB에는 서로 다른 권한 수준을 가진 다섯 가지 기본 제공 역할이 있습니다:

| 역할    | 권한                                     | 설명                  |
|---------|------------------------------------------|-----------------------|
| GOD     | All                                      | 슈퍼유저 (root 전용)  |
| ADMIN   | Read, Write, Create, Delete, Alter, Drop | 전체 관리자           |
| DBA     | Read, Write, Create, Alter               | 데이터베이스 관리자   |
| USER    | Read, Write                              | 표준 사용자           |
| GUEST   | Read                                     | 읽기 전용 접근        |

`GOD`는 process bootstrap identity인 `root`에만 부여됩니다. `CREATE USER ... ROLE GOD`와
`GRANT ROLE GOD`는 거부되며, 애플리케이션 관리자에게는 `ADMIN`을 사용하세요.

### 권한 종류

- **Read**: 데이터 쿼리 (FETCH, GO, MATCH, LOOKUP)
- **Write**: 데이터 수정 (INSERT, UPDATE, DELETE vertex/edge)
- **Create**: 스키마 생성 (CREATE SPACE/TAG/EDGE)
- **Alter**: 스키마 수정 (ALTER TAG/EDGE)
- **Drop**: 스키마 삭제 (DROP SPACE/TAG/EDGE)

### GRANT ROLE

사용자에게 역할을 부여합니다:

```sql
GRANT ROLE <role> TO <username>;
```

**예시:**

```sql
GRANT ROLE ADMIN TO alice;
GRANT ROLE USER TO bob;
GRANT ROLE GUEST TO viewer;
```

### REVOKE ROLE

사용자로부터 역할을 회수합니다:

```sql
REVOKE ROLE <role> FROM <username>;
```

**예시:**

```sql
REVOKE ROLE ADMIN FROM alice;
REVOKE ROLE USER FROM bob;
```

`SHOW ROLES`는 현재 `SHOW USERS`의 별칭으로 같은 사용자/역할 목록을 반환합니다. 역할
정의 자체를 별도 행으로 열거하는 명령은 아직 없습니다.

## 활성 세션

Graph 서비스에 연결된 GOD 또는 ADMIN 사용자는 live session manager의 세션을 조회할
수 있습니다. 결과는 `User`, `Space`만 포함하며 bearer credential인 SessionID는 노출하지
않습니다.

```sql
SHOW SESSIONS;
```

executor를 Graph 서비스 없이 직접 임베드한 경로에는 세션 원본이 없으므로 이 명령은
빈 목록 대신 명시적 unsupported 오류를 반환합니다.

## 기본 사용자

ByoriDB는 첫 시작 시 기본 슈퍼유저를 생성합니다:

- **사용자 이름:** `root`
- **비밀번호:** network server 시작 전에 주입한 `BYORIDB_ROOT_PASSWORD` 값
- **역할:** `GOD`

network server는 `BYORIDB_ROOT_PASSWORD`가 없거나 빈 값이면 시작하지 않으며 credential을
로그에 출력하지 않습니다. 시크릿 매니저에서 주입하고 변경 시 서버를 재시작하세요.

`root`는 process bootstrap 계정이라 `CREATE USER root`로 다시 만들 수 없고, 현재
`ALTER USER root`의 KV 사용자 변경 경로 대상도 아닙니다.

## 모범 사례

1. **root 비밀번호를 안전하게 주입** - 시작 전에 `BYORIDB_ROOT_PASSWORD`를 제공하세요
2. **최소 권한 원칙** - 사용자에게 필요한 최소한의 권한만 부여하세요
3. **ADMIN은 아껴서 사용** - ADMIN 역할은 데이터베이스 관리자에게만 부여하세요
4. **읽기 전용 접근에는 GUEST 사용** - 리포팅 및 분석 사용자에게 적합합니다
5. **정기적인 감사** - 사용자 계정과 역할을 주기적으로 검토하여 적절한 권한을 유지하세요

# 사용자 관리

ByoriDB는 사용자와 권한을 관리하기 위한 역할 기반 접근 제어(RBAC)를 제공합니다.

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

> **참고:** SHOW USERS는 아직 구현되지 않았습니다.

## 역할

ByoriDB에는 서로 다른 권한 수준을 가진 다섯 가지 기본 제공 역할이 있습니다:

| 역할    | 권한                                     | 설명                  |
|---------|------------------------------------------|-----------------------|
| GOD     | All                                      | 슈퍼유저 (root 전용)  |
| ADMIN   | Read, Write, Create, Delete, Alter, Drop | 전체 관리자           |
| DBA     | Read, Write, Create, Alter               | 데이터베이스 관리자   |
| USER    | Read, Write                              | 표준 사용자           |
| GUEST   | Read                                     | 읽기 전용 접근        |

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

> **참고:** SHOW ROLES는 아직 구현되지 않았습니다.

## 기본 사용자

ByoriDB는 첫 시작 시 기본 슈퍼유저를 생성합니다:

- **사용자 이름:** `root`
- **비밀번호:** `BYORIDB_ROOT_PASSWORD`의 값, 또는 시작 시 한 번 로그에 기록되는 생성된 비밀번호
- **역할:** `GOD`

> **보안 경고:** 프로덕션 시작 전에 시크릿 매니저나 보호된 환경에서 `BYORIDB_ROOT_PASSWORD`를 설정하세요.

```sql
ALTER USER root WITH PASSWORD 'your_secure_password';
```

## 모범 사례

1. **root 비밀번호를 명시적으로 설정** - 시작 전에 `BYORIDB_ROOT_PASSWORD`를 제공하세요
2. **최소 권한 원칙** - 사용자에게 필요한 최소한의 권한만 부여하세요
3. **ADMIN은 아껴서 사용** - ADMIN 역할은 데이터베이스 관리자에게만 부여하세요
4. **읽기 전용 접근에는 GUEST 사용** - 리포팅 및 분석 사용자에게 적합합니다
5. **정기적인 감사** - 사용자 계정과 역할을 주기적으로 검토하여 적절한 권한을 유지하세요

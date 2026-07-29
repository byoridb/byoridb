[English](../../../guide/ngql/users.html)

# 사용자와 역할

ByoriDB는 인증 세션에 내장 역할 기반 권한 검사를 제공합니다. 현재 역할은
스페이스 전체에 전역으로 적용되며 아직 스페이스별 테넌트 격리를 제공하지 않습니다.

## Root 부트스트랩

스탠드얼론 서버는 시작 전에 비어 있지 않은 `BYORIDB_ROOT_PASSWORD`를 요구합니다.

```bash
export BYORIDB_ROOT_PASSWORD='value-from-your-secret-manager'
byoridb-server
```

비밀번호는 로그에 출력되지 않습니다. `root` 사용자 이름은 예약되어 있어 새로
만들거나 삭제할 수 없고 nGQL로 변경할 수도 없습니다. Root를 교체하려면
`BYORIDB_ROOT_PASSWORD`를 바꾸고 서버를 재시작하세요.

## 관리 권한 경계

`GOD` 또는 `ADMIN` 세션만 다음을 실행할 수 있습니다.

- `CREATE USER`, `ALTER USER`, `DROP USER`
- `GRANT ROLE`, `REVOKE ROLE`
- `SHOW USER`, `SHOW SESSIONS`
- `BALANCE` 명령

세미콜론으로 묶은 복합 요청 내부나 실제 실행하는 `PROFILE` 내부에 명령이 있어도
같은 검사를 적용합니다. 현재 인터페이스에서 비관리 사용자는 `ALTER USER`로 자기
비밀번호를 바꿀 수 없습니다.

## 사용자 생성

```sql
CREATE USER alice WITH PASSWORD "a-long-random-password";
CREATE USER bob WITH PASSWORD "another-long-password" ROLE USER;
CREATE USER IF NOT EXISTS report_reader WITH PASSWORD "reader-password" ROLE GUEST;
```

비밀번호에는 공백이 아닌 문자가 하나 이상 있어야 합니다. 비밀번호는 Argon2
해시로 저장됩니다. `ROLE` 없이 만든 사용자는 역할을 부여하기 전까지 권한이
없습니다.

사용자 이름은 앞뒤 공백을 제거하지만 그 밖에는 대소문자를 구분합니다. Root
이름은 대소문자와 관계없이 예약됩니다.

`PASSWORD` 키워드 이후의 비밀번호 포함 nGQL은 서버 쿼리 로그와 진단 정보에서
마스킹됩니다. CLI는 입력문을 로컬 `history.txt`에 저장하므로 사용자 관리 후 그
파일을 보호하거나 삭제하세요.

## 사용자 변경과 삭제

```sql
ALTER USER alice WITH PASSWORD "a-new-long-password";
DROP USER alice;
DROP USER IF EXISTS alice;
```

현재 단일 프로세스 graph service에서 비밀번호, 역할, 활성 상태, 삭제를 바꾸면
해당 사용자의 기존 세션이 무효화됩니다. 이후 요청은 다시 인증해야 합니다. 아직
완성되지 않은 분산 모드에서는 클러스터 전체 세션 폐기가 운영상 보장되지 않습니다.

## 내장 역할

| 역할 | 실질 기능 |
| --- | --- |
| `GOD` | 모든 곳에서 read, write, create, alter, drop 및 관리 명령 |
| `ADMIN` | 현재 `GOD`와 같은 권한 집합 및 관리 명령 |
| `DBA` | 모든 곳에서 read, write, create, alter, drop과 사용자 관리는 불가 |
| `USER` | 모든 곳에서 읽기와 그래프 데이터 쓰기 |
| `GUEST` | 모든 곳에서 읽기 전용 |

그래프 데이터 쓰기에는 INSERT, UPDATE, DELETE가 포함됩니다. 내부적으로 ALTER와
스키마 create 검사는 분리되지만 DBA는 둘 다 가집니다. ADMIN은 사용자를 관리하고
특권 역할을 부여할 수 있으므로 완전히 신뢰하는 역할로 취급해야 합니다.

모든 내장 권한 항목은 현재 wildcard 스페이스 `*`를 사용합니다.
`GRANT ... ON <space>` 같은 문법이 없으므로 같은 내장 역할의 사용자 사이에서
별도 스페이스를 보안 경계로 사용하면 안 됩니다.

## 역할 부여와 회수

```sql
GRANT ROLE USER TO alice;
GRANT ROLE DBA TO database_operator;
REVOKE ROLE USER FROM alice;
```

역할 이름은 대문자로 정규화합니다. 유효한 이름은 `GOD`, `ADMIN`, `DBA`, `USER`,
`GUEST`입니다. 역할을 부여하거나 회수하면 로컬 서비스에서 해당 사용자의 현재
세션이 무효화됩니다.

## 조회와 세션

```sql
SHOW USER;
SHOW SESSIONS;
```

`SHOW USER`는 현재 내장 root 레코드만 보고하는 호환성 stub이며 영속 사용자의
신뢰할 수 있는 전체 목록이 아닙니다. `SHOW ROLES`는 내부 표현은 있지만 현재 파서
문법이 없습니다.

`SHOW SESSIONS`는 graph service에 구현되어 사용자와 선택한 스페이스 컬럼만
반환합니다. Bearer session ID는 의도적으로 제외합니다. 세션은 임의의 양수 63-bit
ID를 사용하고 기본 24시간 후 만료됩니다.

모든 session ID를 bearer credential로 취급하세요. 로그에 남기거나 다른 사용자에게
노출하지 마세요.

## 배포 보안

네이티브 gRPC와 HTTP 리스너는 TLS를 종료하지 않으며 ByoriDB는 네트워크 수준
인증 rate limiter를 제공하지 않습니다. 로컬 외부 배포는 신뢰할 수 있는 TLS
endpoint, 방화벽 또는 네트워크 정책, 경계 rate limiting 뒤에 두세요. Root와 사용자
자격 증명은 저장소 파일이나 명령줄 인자 대신 시크릿 매니저에 보관하세요.

# 보안 정책

[English](SECURITY.md)

## 취약점 비공개 제보

의심되는 취약점을 공개 issue, discussion, Pull Request, 로그 또는 대화 내용에
공개하지 마세요.

저장소 **Security** 탭의 GitHub 비공개 취약점 제보 기능을 사용하거나
[비공개 security advisory](https://github.com/byoridb/byoridb/security/advisories/new)를
여세요. 재현과 평가에 필요한 정보만 포함하세요.

- 영향을 받는 revision 또는 release
- 배포 방식과 운영체제
- 영향을 받는 인터페이스(HTTP, gRPC, CLI, storage, backup 또는 cluster)
- 사전 조건과 최소 재현 절차
- 기대한 동작과 관찰한 동작
- 기밀성, 무결성 또는 가용성에 미치는 잠재적 영향

실제 credential과 비공개 데이터는 제거하세요. GitHub에서 비공개 제보 기능을
제공하지 않으면 취약점 세부사항을 포함하지 않은 공개 issue를 열고 maintainer에게
비공개 채널을 요청하세요.

Maintainer는 제보를 검증하고 영향받는 버전을 평가한 뒤 필요한 경우 수정과 공개를
조율합니다. 현재 이 프로젝트는 고정된 보안 대응 SLA를 약속하지 않습니다.

## 지원 버전

| 버전 | 보안 지원 |
|---|---|
| Review된 현재 `main` revision | 보안 수정 대상 |
| Tagged release와 이전 revision | 유지되는 backport branch 또는 지원 약속 없음 |

Release tag는 유지보수되는 지원 계열이 아니라 변경되지 않는 snapshot입니다.
`main`에 merge된 수정이 기존 tag나 binary에 backport되었다고 가정하지 마세요.

## 현재 보안 모델

### Credential과 authentication

Standalone `byoridb-server`는 비어 있지 않은 `BYORIDB_ROOT_PASSWORD`를 요구하며
없으면 시작을 거부합니다. 이 값은 배포 secret manager에 저장하고 source control,
container image 또는 `.env` 파일에 commit하지 마세요.

`root` identity는 예약되어 있습니다. `CREATE USER`, `ALTER USER`, `DROP USER`로
대체하거나 변경할 수 없습니다. `BYORIDB_ROOT_PASSWORD`를 변경하고 서버를
재시작하여 root credential을 교체하세요. 재시작하면 해당 프로세스의 모든 session이
무효화됩니다.

영속 사용자 비밀번호는 salt가 적용된 Argon2 hash로 저장됩니다. HTTP와 gRPC의 인증
실패 응답은 일반화되어 있어 caller가 존재하지 않는 계정, 비활성 계정, 잠긴 계정,
잘못된 비밀번호를 직접 구별할 수 없습니다.

### 권한

ByoriDB는 statement 단위 role 검사를 적용합니다. 사용자 및 role 관리,
`SHOW USERS`, `SHOW ROLES`, `SHOW SESSIONS`, `BALANCE`는 `GOD` 또는 `ADMIN`을
요구합니다. Compound statement와 `PROFILE`이 실행하는 내부 statement도 재귀적으로
검사합니다.

`SHOW USERS`는 password hash 없이 built-in root와 영속 사용자를 반환합니다.
`SHOW SESSIONS`는 username과 선택된 space를 반환하지만 bearer session ID는
제외합니다.

기본 role은 `GOD`, `ADMIN`, `DBA`, `USER`, `GUEST`입니다. 현재 permission은 wildcard
space `*`를 사용하며 query language는 space 범위를 지정하는 `GRANT`를 제공하지
않습니다. 따라서 기본 RBAC는 space별 tenant 격리 경계가 아닙니다.

### Session과 diagnostics

Session ID는 암호학적으로 무작위인 양의 63-bit bearer credential입니다. 기본 sliding
session 수명은 24시간입니다. Standalone 서버의 HTTP와 gRPC는 한 프로세스 안에서
authentication 및 session 상태를 공유합니다. 비밀번호나 role 변경과 사용자 삭제는
그 사용자의 local session을 폐기합니다. 서로 다른 프로세스 사이에는 session 생성과
폐기가 조정되지 않습니다.

Session ID를 로그, analytics, trace 또는 issue report에 넣지 마세요.
`X-ByoriDB-Session-Id` header를 사용하는 HTTP 작업은 다른 bearer-token 흐름과 같은
수준으로 보호해야 합니다.

`GET /api/v1/diagnostics/queries`는 이 header에 live `GOD` 또는 `ADMIN` session을
요구합니다. 응답에는 안전한 query metadata만 포함되며 raw query text와 session ID는
제외됩니다. `/health`, `/ready`, `/metrics`, `/api/v1/metrics`는 인증되지 않으므로
가용성이나 운영 metadata가 민감하다면 network 또는 proxy layer에서 제한하세요.

## 알려진 배포 경계

- **Native transport encryption 없음:** HTTP와 gRPC는 plaintext입니다. 신뢰할 수
  있는 ingress, proxy 또는 service mesh에서 TLS를 종료하고 backend hop을 보호하세요.
- **범용 login rate limiter 없음:** in-process account lockout은 source별 throttling을
  대체하지 않습니다. Network edge에서 connection 및 login limit을 적용하세요.
- **Space별 grant 표면 없음:** 서로 신뢰하지 않는 tenant 사이에 강한 격리가
  필요하면 분리된 trusted deployment를 사용하세요.
- **Process-local session:** standalone Graph service를 shared session authority가
  있는 것처럼 수평 확장하지 마세요.
- **미완성 distributed operation:** cluster와 Raft component는 있지만 storage peer
  bootstrap, deployment wiring, multi-node 운영 검증은 아직 미완성입니다.
- **Relaxed durability의 data loss 가능성:** `BYORIDB_DURABILITY=relaxed`, `none`,
  `eventual`은 commit별 fsync를 비활성화합니다. 다시 적재할 수 있는 bulk import에만
  사용하세요.

## 배포 checklist

- Review된 최신 revision을 실행하고 dependency advisory를 모니터링하세요.
- 고유하고 entropy가 충분한 `BYORIDB_ROOT_PASSWORD`를 secret manager에서
  주입하세요.
- 가능하면 listener를 private interface에 bind하고 두 API를 firewall로 제한하세요.
- Edge에서 TLS를 종료하고 request-size control과 rate limit을 적용하세요.
- 인증되지 않은 health, readiness, metrics route를 제한하세요.
- 서로 신뢰하지 않는 tenant 격리에 기본 role을 의존하지 마세요.
- 모든 외부 logging layer에서 session header, 인증 응답, password-bearing query body를
  redact하세요.
- Data directory와 backup을 least-privilege filesystem 권한으로 보호하세요. Backup에는
  current data와 temporal history가 포함됩니다.
- Serving workload에는 immediate durability를 유지하고 backup restore를 test하세요.
- 환경 변수를 변경하고 재시작하여 root secret을 교체하세요. 모든 client가 다시
  연결해야 합니다.

## 범위

이 정책은 [byoridb/byoridb](https://github.com/byoridb/byoridb) 저장소에서 관리하는
code를 대상으로 합니다. 별도 agent-memory 제품에 대한 report는
[byoridb/byori](https://github.com/byoridb/byori) 저장소로 보내세요.

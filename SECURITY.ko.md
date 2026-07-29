# 보안 정책

[English](SECURITY.md)

## 취약점 비공개 제보

의심되는 취약점을 public issue, discussion, Pull Request, log excerpt 또는 chat
transcript에 공개하지 마세요.

저장소 **Security** tab의 GitHub private vulnerability reporting 흐름을 사용하거나
[private security advisory report](https://github.com/byoridb/byoridb/security/advisories/new)를
여세요. 재현과 평가에 필요한 정보만 포함하세요.

- 영향을 받는 revision 또는 release
- 배포 mode와 운영체제
- 영향을 받는 interface(HTTP, gRPC, CLI, storage, backup 또는 cluster component)
- 사전 조건과 최소 재현 절차
- 기대한 동작과 관찰한 동작
- confidentiality, integrity 또는 availability에 미치는 잠재적 영향
- 알고 있다면 제안 mitigation

실제 credential과 private data는 제거하세요. GitHub에서 private reporting control을
제공하지 않으면 취약점 세부사항을 전혀 포함하지 않은 public issue를 열고
maintainer에게 private channel을 요청하세요.

Maintainer는 제보를 검증하고 severity와 영향 version을 평가한 뒤 필요한 경우 fix와
disclosure를 조율합니다. 대응 및 release 일정은 영향과 maintainer 가용성에 따라
달라지며 현재 이 프로젝트는 고정된 security-response SLA를 약속하지 않습니다.

## 지원 version

| Version | Security 지원 |
|---|---|
| Review된 현재 `main` revision | Security fix 대상 |
| Tagged release와 이전 revision | 유지되는 security-support branch 또는 backport 약속 없음 |

Release tag는 유지보수되는 semantic-version support line이 아니라 immutable
snapshot입니다. 관련 fix를 포함하는 review된 현재 revision을 사용하세요. `main`에
merge된 fix가 기존 tag나 binary에 backport되었다고 가정하지 마세요.

## 현재 security model

### Root credential

standalone `byoridb-server`는 비어 있지 않은 `BYORIDB_ROOT_PASSWORD`를 요구하며
없으면 시작을 거부합니다. 이 값은 log에 기록하지 않습니다. Deployment secret
manager에 저장하고 source control, container image 또는 `.env` 파일에 commit하지
마세요.

`root` identity는 reserved이며 `CREATE USER`, `ALTER USER`, `DROP USER`로 비밀번호를
바꿀 수 없습니다. `BYORIDB_ROOT_PASSWORD`를 변경하고 서버를 재시작하여
rotation하세요. 재시작하면 해당 프로세스의 모든 live session이 무효화됩니다.

사용자 비밀번호는 Argon2 hash로 저장합니다. 빈 값과 공백만 있는 비밀번호는
거부합니다. Authentication failure는 외부에 generic error를 반환하므로 caller가
unknown, disabled, wrong-password account를 응답으로 직접 구별할 수 없습니다.

### 권한

ByoriDB는 statement-level role check를 적용합니다. User/role 관리, `SHOW USER`,
`SHOW SESSIONS`, `BALANCE`는 `GOD` 또는 `ADMIN`이 필요하며 nested
compound/profiled statement도 recursive하게 검사합니다. 현재 `SHOW USER` 결과는
built-in root placeholder입니다. `SHOW SESSIONS`는 active user와 선택된 space를
나열하지만 bearer session ID를 생략합니다. Public parser는 `SHOW USERS`와
`SHOW ROLES`를 허용하지 않습니다.

기본 role은 `GOD`, `ADMIN`, `DBA`, `USER`, `GUEST`입니다. 현재 permission entry는
wildcard space `*`를 대상으로 합니다. Internal model은 space를 표현할 수 있지만
public query language에는 현재 space-scoped `GRANT` operation이 없습니다. 따라서
기본 RBAC model은 multi-tenant 또는 per-space isolation 경계가 아닙니다.

### Session

Session ID는 random positive 63-bit 값이며 bearer credential로 취급해야 합니다.
기본 session lifetime은 24시간입니다. Authentication/session state는 memory에
존재하며 동일한 서버 프로세스 안에서만 HTTP와 gRPC가 공유합니다.

사용자의 비밀번호나 role 변경, 사용자 비활성화 또는 삭제는 local process의 해당
사용자 session을 무효화합니다. 별도 ByoriDB process 사이에는 session 생성 및 폐기가
조정되지 않습니다. Process를 재시작하면 그 process의 모든 session이 무효화됩니다.

Session ID를 application log, analytics, trace 또는 error report에 넣지 마세요. 현재
HTTP sign-out endpoint는 URL path에 session ID를 포함하므로 reverse-proxy access
log에서 해당 route를 redact하거나 제외해야 합니다.

### Diagnostics와 log

`GET /api/v1/diagnostics/queries`는
`Authorization: Bearer <session-id>` header에 live `GOD` 또는 `ADMIN` session을
요구합니다. 응답은 raw session ID를 제외하고 `PASSWORD` keyword 이후의 query
text를 redact합니다. Authentication과 invalid-session response는 credential을
의도적으로 반사하지 않습니다.

이 보호는 reverse proxy, service mesh, client 또는 operator가 추가한 log를
제어하지 않습니다. 해당 system을 별도로 설정하세요. `/health`, `/ready`,
`/metrics`, `/api/v1/metrics`에는 현재 인증이 없습니다. Availability 또는 운영
metadata가 민감하면 network/proxy layer에서 제한하세요.

## 알려진 배포 경계

- **Native transport encryption 없음:** HTTP와 gRPC는 plaintext입니다. 신뢰할 수
  있는 ingress, proxy 또는 service mesh에서 TLS를 terminate하고 필요한 경우 backend
  hop도 인증하세요.
- **범용 login rate limiter 없음:** in-process failed-attempt tracking은 per-source
  throttling을 대체하지 않습니다. Network edge에서 connection limit과 login rate
  limit을 적용하세요.
- **Per-space grant 표면 없음:** 기본 role은 모든 space에 적용됩니다. 강한 tenant
  isolation이 필요하면 서로 분리된 trusted deployment를 사용하세요.
- **Process-local session:** instance 사이에 revocation과 expiry가 분산되지 않습니다.
  현재 standalone Graph service를 shared session authority가 있는 것처럼 horizontal
  scale하지 마세요.
- **미완성 distributed operation:** cluster configuration과 custom Raft component는
  있지만 Storage peer bootstrap, deployment wiring, multi-node 운영 검증은
  미완성입니다. Distributed mode는 production security/availability 경계가 아닙니다.
- **Public operational endpoint:** health, readiness, metrics endpoint는 session을
  요구하지 않습니다.
- **Relaxed durability의 data loss 가능성:** `BYORIDB_DURABILITY=relaxed`, `none`,
  `eventual`은 per-commit fsync를 비활성화합니다. Reload 가능한 bulk import에만
  사용하고 steady-state serving에는 사용하지 마세요.

## 배포 checklist

- Review된 최신 revision을 실행하고 dependency advisory를 모니터링하세요.
- Unique하고 entropy가 충분한 `BYORIDB_ROOT_PASSWORD`를 secret manager에서
  주입하세요.
- 가능하면 listener를 loopback/private interface에 bind하고 두 기본 port를 모두
  firewall로 제한하세요.
- Edge에서 TLS termination, authentication, request-size control, rate limit을
  적용하세요.
- 인증되지 않은 health/metrics route를 trusted monitoring system으로 제한하세요.
- 기본 role로 서로 신뢰하지 않는 tenant 또는 space를 격리하지 마세요.
- Multi-instance session/data coordination을 별도로 설계하고 검증하지 않았다면 하나의
  trusted standalone server process만 사용하세요.
- 모든 외부 logging layer에서 session-bearing path, authorization header,
  password-bearing query body를 redact하세요.
- Data directory와 backup을 least-privilege filesystem access로 보호하세요. Backup에는
  current data와 temporal history가 모두 들어 있습니다.
- Serving workload에는 immediate durability를 유지하고 backup restore를 test하며
  active data 교체 전에 restored database를 검증하세요.
- 환경 변수를 바꾸고 재시작하여 root secret을 rotation하세요. 모든 session이 다시
  연결되어야 합니다.

## 범위

이 정책은 [byoridb/byoridb](https://github.com/byoridb/byoridb) 저장소에서 관리하는
code를 대상으로 합니다. Upstream dependency의 취약점은 적절한 경우 해당 upstream
project에도 제보해야 합니다. 별도의 agent-memory 제품에 대한 report는
[byoridb/byori](https://github.com/byoridb/byori) 저장소로 보내세요.

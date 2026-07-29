# 보안

> [English](../../operations/security.html) | **한국어**

ByoriDB에는 애플리케이션 계층 인증과 role 검사가 있지만 완전한 프로덕션 보안 경계를
제공하지는 않습니다. 운영자는 아래의 network와 transport 통제를 적용해야 합니다.

취약점 신고와 권위 있는 제약 목록은 저장소의
[보안 정책](https://github.com/byoridb/byoridb/blob/main/SECURITY.ko.md)을 참고하세요.

## Root credential

Standalone 서버는 `BYORIDB_ROOT_PASSWORD`가 비어 있지 않은 값으로 설정되지 않으면
시작을 거부합니다.

```bash
export BYORIDB_ROOT_PASSWORD='use-a-secret-manager-value'
cargo run --bin byoridb-server --release
```

비밀번호는 출력되지 않습니다. 관리되는 환경 secret을 변경하고 서버를 재시작해 root를
rotation하세요. `ALTER USER root`는 거부됩니다. 비밀번호를 shell history, process argument,
image layer, commit된 Compose/Kubernetes 파일에 직접 넣지 마세요.

`AuthManager`의 embedded 사용자가 root credential을 생략하면 fail-closed fallback이
동작하지만, 그 무작위 값은 의도적으로 공개되지 않으며 정상 설정을 대신하지 않습니다.

## Role

| Role | 일반 문장 권한 | 특별 관리 작업 |
|---|---|---|
| `GOD` | Read, write, create/alter, drop | 가능 |
| `ADMIN` | Read, write, create/alter, drop | 가능 |
| `DBA` | Read, write, create/alter | User/session/balance 관리 불가 |
| `USER` | Read와 write | 불가 |
| `GUEST` | Read only | 불가 |

현재 의미론의 중요 사항:

- 기본 entry는 `space="*"`에 적용되며 공개 space-scoped ACL 문법이 없습니다.
- `Write`는 insert, update, delete를 모두 포함합니다.
- 일반 `ALTER`는 현재 `Create`를 검사하고 `Alter` permission variant를 별도로 집행하지 않습니다.
- 다른 role이 `Create`를 가져도 user/role 변경, `BALANCE`, `SHOW USER`,
  `SHOW SESSIONS`는 GOD 또는 ADMIN만 가능합니다. 현재 `SHOW USER`는 root 전용
  placeholder입니다. `SHOW SESSIONS`는 active user와 선택된 space를 나열하지만
  bearer session ID는 생략합니다. Public parser는 `SHOW USERS`와 `SHOW ROLES`를
  모두 허용하지 않습니다.
- 인가는 compound statement와 실제 실행되는 `PROFILE`을 재귀 검사합니다.
  실행하지 않는 `EXPLAIN`은 read-only입니다.

현재 role model을 tenant별 격리로 설명하면 안 됩니다.

## Session과 revoke

Session ID는 무작위 양수 signed 64-bit 값입니다. HTTP는 JavaScript client의 정밀도
손실을 막기 위해 decimal string으로 표현합니다. Session ID를 bearer secret으로 취급하세요.

Password, role, enabled state, user 변경은 같은 서버 프로세스 안에서 해당 사용자의
session을 무효화합니다. Standalone 서버 안의 HTTP와 gRPC는 하나의 인증 상태를 공유합니다.
이 보장은 여러 서버 프로세스에는 적용되지 않으며 cluster-wide revoke는 미구현입니다.

다음을 로그에 남기지 마세요.

- Session ID
- `/api/v1/session/{id}` path
- 인증 request body
- `PASSWORD`를 포함하는 query

ByoriDB 자체 diagnostics는 credential query와 active-query session ID를 redaction하지만,
reverse proxy와 ingress access log는 별도로 설정해야 합니다.

## HTTP endpoint 접근

| Endpoint | 접근 |
|---|---|
| `POST /api/v1/session` | 공개 인증 endpoint |
| `POST /api/v1/query` | JSON request에 session ID 포함 |
| `POST /api/v1/query/json` | JSON request에 session ID 포함 |
| `DELETE /api/v1/session/{id}` | 해당 session이 자기 자신을 sign out |
| `GET /api/v1/diagnostics/queries` | GOD/ADMIN의 `Authorization: Bearer <session-id>` 필요 |
| `GET /health`, `GET /ready` | 공개 health signal |
| `GET /metrics`, `GET /api/v1/metrics` | 공개; network/proxy 계층에서 보호 |

Diagnostics 응답은 bearer session ID를 생략하고 비밀번호를 포함한 query text를 redaction합니다.

## 필수 배포 통제

1. HTTP와 gRPC를 사설망 안에 둡니다.
2. 신뢰할 수 있는 proxy/ingress에서 TLS 또는 mTLS를 종료하고, 내부 hop도 같은 수준으로
   신뢰할 수 없다면 서버까지 암호화합니다.
3. 인증 endpoint에 connection/request rate limit을 적용합니다.
4. Health와 metrics endpoint를 의도한 monitoring system으로 제한합니다.
5. Root credential을 secret manager에 저장하고 계획된 재시작으로 rotation합니다.
6. Filesystem과 backup 접근을 제한합니다. Backup에는 raw DB data와 password hash가 있습니다.
7. Credential을 기록하지 않으면서 login 실패와 resource saturation을 모니터링합니다.

## 알려진 보안 gap

- Native HTTP/gRPC TLS 없음.
- 실효성 있는 per-IP/global login rate limit과 Argon2 worker pool 제한 없음.
- Space-scoped grant 없음.
- Cluster-wide session/cache coordination 없음.
- Metrics endpoint 인증 없음.
- HTTP sign-out URL에 bearer 성격의 ID 포함.

이 항목은 숨겨진 가정이 아니라 명시된 roadmap입니다.
[프로젝트 계획](https://github.com/byoridb/byoridb/blob/main/docs/PLAN.ko.md)을 참고하세요.

# 보안

[English](../../operations/security.html)

ByoriDB는 애플리케이션 계층의 인증과 역할 검사를 제공하지만, 완전한 운영 보안
경계를 자체적으로 제공하지는 않습니다. 운영자는 아래의 네트워크 및 전송 계층
보호를 별도로 구성해야 합니다.

취약점 제보 절차와 권위 있는 제한 사항은 저장소의
[보안 정책](https://github.com/byoridb/byoridb/blob/main/SECURITY.ko.md)을 참고하세요.

## Root credential

스탠드얼론 서버는 `BYORIDB_ROOT_PASSWORD`가 없거나 빈 문자열이면 시작을
거부합니다. 공백으로만 구성된 값에 대한 별도 강도 검사는 현재 수행하지 않습니다.

```bash
export BYORIDB_ROOT_PASSWORD='use-a-secret-manager-value'
cargo run --locked --release -p byoridb --bin byoridb-server
```

비밀번호 값은 로그에 출력되지 않습니다. Root credential은 관리되는 환경 변수 값을
변경하고 서버를 재시작하여 교체하세요. `root` identity는 예약되어 있어 nGQL의
`CREATE USER`, `ALTER USER`, `DROP USER`로 만들거나 변경하거나 삭제할 수 없습니다.

영속 사용자 비밀번호는 salt가 적용된 Argon2 hash로 저장됩니다. 현재 사용자 관리
경로는 별도의 최소 길이나 강도 정책을 강제하지 않으므로, 운영 정책에서 고유하고
entropy가 충분한 비밀번호를 요구해야 합니다.

## 역할과 권한

| 역할 | 일반 statement 권한 | 특별 관리 기능 |
|---|---|---|
| `GOD` | Read, write, create/alter, drop | 사용 가능 |
| `ADMIN` | Read, write, create/alter, drop | 사용 가능 |
| `DBA` | Read, write, create/alter | 사용자/session/balance 관리 불가 |
| `USER` | Read, write | 불가 |
| `GUEST` | Read only | 불가 |

현재 동작에서 유의할 점은 다음과 같습니다.

- 기본 permission은 wildcard space `*`에 적용되며, 공개된 space 범위 ACL 문법은
  없습니다.
- `Write`는 graph data의 insert, update, delete를 포함합니다.
- 사용자와 role 변경, `BALANCE`, `SHOW USERS`, `SHOW ROLES`, `SHOW SESSIONS`는
  다른 role이 `Create` 권한을 가지고 있어도 `GOD` 또는 `ADMIN`만 실행할 수
  있습니다.
- `SHOW USERS`는 password hash 없이 built-in root와 영속 사용자를 반환합니다.
  `SHOW ROLES`는 현재 같은 사용자/role 목록을 반환합니다. `SHOW SESSIONS`는
  username과 선택된 space만 반환하고 bearer session ID를 제외합니다.
- Compound statement와 실행되는 `PROFILE`의 내부 statement도 재귀적으로 권한을
  검사합니다. 실행하지 않는 `EXPLAIN`은 read-only로 처리됩니다.

현재 role 모델을 space별 tenant 격리 경계로 사용하면 안 됩니다.

## Session과 폐기

Session ID는 암호학적으로 무작위인 양의 63-bit 값입니다. HTTP는 JavaScript 정밀도
손실을 피하기 위해 이를 decimal string으로 표현합니다. Session ID는 bearer
credential로 취급하세요.

비밀번호, role, enabled 상태 또는 사용자 삭제는 같은 서버 프로세스에 있는 해당
사용자의 session을 폐기합니다. 스탠드얼론 서버의 HTTP와 gRPC는 한 authentication
상태를 공유합니다. 별도 프로세스 사이에는 session 생성과 폐기가 조정되지 않습니다.

다음 값은 로그에 남기지 마세요.

- Session ID와 `X-ByoriDB-Session-Id` header
- 인증 요청 body
- `PASSWORD`를 포함한 query

현재 ByoriDB의 query log와 active-query diagnostics는 raw query text나 session ID
대신 제한된 metadata만 유지합니다. Reverse proxy와 ingress log도 민감한 header와
body를 제외하도록 별도로 설정해야 합니다.

## HTTP endpoint 접근

| Endpoint | 접근 방식 |
|---|---|
| `POST /api/v1/session` | 공개 인증 endpoint |
| `POST /api/v1/query` | JSON request의 session ID |
| `POST /api/v1/query/json` | JSON request의 session ID |
| `DELETE /api/v1/session` | live session을 `X-ByoriDB-Session-Id`에 전달하여 본인 session 종료 |
| `GET /api/v1/diagnostics/queries` | `GOD`/`ADMIN`의 live `X-ByoriDB-Session-Id` |
| `GET /health`, `GET /ready` | 공개 health signal |
| `GET /metrics`, `GET /api/v1/metrics` | 공개 endpoint, network/proxy에서 보호 필요 |

Diagnostics 응답은 `id`, `query_type`, `query_length_bytes`, `space`,
`started_at_ms`만 포함하며 raw query text와 bearer session ID를 제외합니다.

## 필수 배포 보호

1. HTTP와 gRPC를 private network 안에 두세요.
2. 신뢰할 수 있는 proxy 또는 ingress에서 TLS나 mTLS를 종료하고, backend network가
   같은 수준으로 신뢰되지 않으면 backend hop도 암호화하세요.
3. 인증 endpoint에 connection 및 request rate limit을 적용하세요.
4. Health와 metrics endpoint를 필요한 monitoring system으로 제한하세요.
5. Root credential은 secret manager에 저장하고 계획된 재시작으로 교체하세요.
6. Data directory와 backup 접근 권한을 제한하세요. Backup에는 원본 database data와
   password hash가 포함됩니다.
7. Credential을 기록하지 않으면서 로그인 실패와 resource 포화를 모니터링하세요.

## 알려진 보안 제한

- Native HTTP/gRPC TLS 없음
- 효과적인 per-IP/global 로그인 rate limit 또는 제한된 Argon2 worker pool 없음
- Space별 grant 없음
- Cluster 전체 session/cache 조정 없음
- 인증되지 않은 metrics endpoint

이 항목은 숨겨진 가정이 아니라 현재의 roadmap 제약입니다. 자세한 내용은
[프로젝트 계획](https://github.com/byoridb/byoridb/blob/main/docs/PLAN.ko.md)을
참고하세요.

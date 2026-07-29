# Security

> **English** | [한국어](../ko/operations/security.html)

ByoriDB has application-layer authentication and role checks, but it does not
yet provide a complete production security perimeter. Operators must supply the
network and transport controls described below.

For vulnerability reporting and the authoritative limitations list, see the
repository [security policy](https://github.com/byoridb/byoridb/blob/main/SECURITY.md).

## Root credential

The standalone server refuses to start unless `BYORIDB_ROOT_PASSWORD` is set to
a non-blank value.

```bash
export BYORIDB_ROOT_PASSWORD='use-a-secret-manager-value'
cargo run --bin byoridb-server --release
```

The password is never printed. Rotate root by changing the managed environment
secret and restarting the server; `ALTER USER root` is rejected. Avoid putting
the password directly in shell history, process arguments, image layers, or
checked-in Compose/Kubernetes files.

Embedded users of `AuthManager` have a fail-closed fallback when they omit the
root credential, but that random value is deliberately undisclosed and is not a
substitute for configuration.

## Roles

| Role | Ordinary statement permissions | Special administration |
|---|---|---|
| `GOD` | Read, write, create/alter, drop | Yes |
| `ADMIN` | Read, write, create/alter, drop | Yes |
| `DBA` | Read, write, create/alter | No user/session/balance administration |
| `USER` | Read and write | No |
| `GUEST` | Read only | No |

Important current semantics:

- Built-in entries apply to `space="*"`; there is no public space-scoped ACL
  syntax.
- `Write` covers insert, update, and delete.
- Ordinary `ALTER` currently checks `Create`; it is not separately enforced by
  the `Alter` permission variant.
- User and role mutation, `BALANCE`, `SHOW USER`, and `SHOW SESSIONS` require
  GOD or ADMIN even when another role has `Create`. `SHOW USER` is currently a
  root-only placeholder. `SHOW SESSIONS` lists active users and selected spaces
  but omits bearer session IDs. The public parser accepts neither `SHOW USERS`
  nor `SHOW ROLES`.
- Authorization recursively inspects compound statements and an executing
  `PROFILE`. `EXPLAIN` without execution is read-only.

Do not describe the current role model as per-tenant isolation.

## Sessions and revocation

Session IDs are random positive signed 64-bit values. HTTP represents them as
decimal strings so JavaScript clients do not lose integer precision. Treat a
session ID as a bearer secret.

Password, role, enabled-state, and user changes invalidate that user's sessions
in the same server process. HTTP and gRPC share one authentication state inside
the standalone server. These guarantees do not extend across multiple server
processes; cluster-wide revocation is not implemented.

Avoid logging:

- session IDs;
- `/api/v1/session/{id}` paths;
- authentication request bodies;
- queries containing `PASSWORD`.

ByoriDB redacts credential-bearing queries and active-query session IDs in its
own diagnostics, but reverse proxies and ingress access logs require separate
configuration.

## HTTP endpoint access

| Endpoint | Access |
|---|---|
| `POST /api/v1/session` | Public authentication endpoint |
| `POST /api/v1/query` | Session ID in the JSON request |
| `POST /api/v1/query/json` | Session ID in the JSON request |
| `DELETE /api/v1/session/{id}` | The session signs itself out |
| `GET /api/v1/diagnostics/queries` | `Authorization: Bearer <session-id>` from GOD/ADMIN |
| `GET /health`, `GET /ready` | Public health signals |
| `GET /metrics`, `GET /api/v1/metrics` | Public; protect at the network/proxy layer |

The diagnostics response omits bearer session IDs and redacts password-bearing
query text.

## Required deployment controls

1. Keep HTTP and gRPC on a private network.
2. Terminate TLS or mTLS at a trusted proxy/ingress and encrypt the hop to the
   server whenever that network is not equally trusted.
3. Apply connection and request rate limits to the authentication endpoint.
4. Restrict health and metrics endpoints to intended monitoring systems.
5. Store root credentials in a secret manager and rotate them through a planned
   restart.
6. Restrict filesystem and backup access; backups contain raw database data and
   password hashes.
7. Monitor failed logins and resource saturation without logging credentials.

## Known security gaps

- No native HTTP/gRPC TLS.
- No effective per-IP/global login rate limit or bounded Argon2 worker pool.
- No space-scoped grants.
- No cluster-wide session/cache coordination.
- Metrics endpoints are unauthenticated.
- The HTTP sign-out route includes the bearer-like ID in the URL.

These are tracked roadmap items, not hidden assumptions. See
[the project plan](https://github.com/byoridb/byoridb/blob/main/docs/PLAN.md).

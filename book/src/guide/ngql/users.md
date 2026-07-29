[한국어](../../ko/guide/ngql/users.html)

# Users and roles

ByoriDB provides built-in role-based authorization for authenticated sessions.
The current roles are global across spaces; they do not yet provide per-space
tenant isolation.

## Root bootstrap

The standalone server requires a non-blank `BYORIDB_ROOT_PASSWORD` before
startup:

```bash
export BYORIDB_ROOT_PASSWORD='value-from-your-secret-manager'
byoridb-server
```

The password is never printed to logs. The `root` username is reserved, cannot
be created or dropped, and cannot be altered through nGQL. Rotate root by
changing `BYORIDB_ROOT_PASSWORD` and restarting the server.

## Administrative boundary

Only a session with `GOD` or `ADMIN` may execute:

- `CREATE USER`, `ALTER USER`, and `DROP USER`;
- `GRANT ROLE` and `REVOKE ROLE`;
- `SHOW USER` and `SHOW SESSIONS`; and
- `BALANCE` commands.

These checks also apply when a command is inside a semicolon-separated compound
request or inside executing `PROFILE`. Non-administrators cannot change their
own password with `ALTER USER` in the current interface.

## Create users

```sql
CREATE USER alice WITH PASSWORD "a-long-random-password";
CREATE USER bob WITH PASSWORD "another-long-password" ROLE USER;
CREATE USER IF NOT EXISTS report_reader WITH PASSWORD "reader-password" ROLE GUEST;
```

A password must contain at least one non-whitespace character. Passwords are
stored as Argon2 hashes. A user created without `ROLE` has no permissions until
a role is granted.

Usernames are trimmed but otherwise case-sensitive. The root name is reserved
case-insensitively.

Password-bearing nGQL is redacted from server query logs and diagnostics after
the `PASSWORD` keyword. The CLI still saves entered statements to its local
`history.txt`; protect or remove that file after user-management work.

## Change or remove users

```sql
ALTER USER alice WITH PASSWORD "a-new-long-password";
DROP USER alice;
DROP USER IF EXISTS alice;
```

On the current single-process graph service, password, role, enablement, and
deletion changes invalidate that user's existing sessions. A later request must
authenticate again. Cluster-wide session revocation is not yet an operational
guarantee in the incomplete distributed mode.

## Built-in roles

| Role | Effective capability |
| --- | --- |
| `GOD` | Read, write, create, alter, and drop everywhere; administrative commands |
| `ADMIN` | The same current permission set as `GOD`; administrative commands |
| `DBA` | Read, write, create, and alter everywhere; no drop or user administration |
| `USER` | Read and graph-data write everywhere |
| `GUEST` | Read-only everywhere |

Graph-data write includes INSERT, UPDATE, and DELETE. `ALTER` and schema-create
checks are separate internally, but DBA has both. ADMIN should be treated as a
fully trusted role because it can manage users and grant privileged roles.

Every built-in permission entry currently uses the wildcard space `*`. There is
no syntax such as `GRANT ... ON <space>`, so do not use separate spaces as a
security boundary between users with the same built-in role.

## Grant and revoke roles

```sql
GRANT ROLE USER TO alice;
GRANT ROLE DBA TO database_operator;
REVOKE ROLE USER FROM alice;
```

Role names are normalized to uppercase. Valid names are `GOD`, `ADMIN`, `DBA`,
`USER`, and `GUEST`. Granting or revoking a role invalidates the affected user's
current sessions in the local service.

## Introspection and sessions

```sql
SHOW USER;
SHOW SESSIONS;
```

`SHOW USER` is currently a compatibility stub that reports only the built-in
root record; it is not an authoritative list of persisted users. `SHOW ROLES`
is represented internally but has no parser syntax today.

`SHOW SESSIONS` is implemented in the graph service and returns only user and
selected-space columns. It deliberately omits bearer session IDs. Sessions use
random positive 63-bit identifiers and expire after 24 hours by default.

Treat every session ID as a bearer credential. Do not put it in logs or expose
it to another user.

## Deployment security

The native gRPC and HTTP listeners do not terminate TLS, and ByoriDB does not
provide a network-level authentication rate limiter. Put non-local deployments
behind a trusted TLS endpoint, firewall or network policy, and edge rate
limiting. Store root and user credentials in a secret manager rather than in
repository files or command-line arguments.

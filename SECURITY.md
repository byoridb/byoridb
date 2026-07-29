# Security policy

[한국어](SECURITY.ko.md)

## Report a vulnerability privately

Please do not disclose a suspected vulnerability in a public issue, discussion,
pull request, log excerpt, or chat transcript.

Use GitHub's private vulnerability reporting flow from the repository's
**Security** tab, or open
[a private security advisory report](https://github.com/byoridb/byoridb/security/advisories/new).
Include only enough information to reproduce and assess the issue:

- affected revision or release;
- deployment mode and operating system;
- affected interface (HTTP, gRPC, CLI, storage, backup, or cluster component);
- prerequisites and a minimal reproduction;
- expected and observed behavior;
- potential confidentiality, integrity, or availability impact;
- any suggested mitigation, if known.

Remove real credentials and private data. If GitHub does not offer the private
reporting control, open a public issue containing no vulnerability details and
ask a maintainer for a private channel.

Maintainers will validate the report, assess severity and affected versions,
and coordinate a fix and disclosure when appropriate. Response and release
timing depends on impact and maintainer availability; this project does not
currently promise a fixed security-response SLA.

## Supported versions

| Version | Security support |
|---|---|
| Reviewed current `main` revision | Target for security fixes |
| Tagged releases and older revisions | No maintained security-support branch or backport promise |

Release tags are immutable snapshots, not maintained semantic-version support
lines. Use a reviewed current revision that contains the relevant fix. Do not
assume that a fix merged to `main` has been backported to an existing tag or
binary.

## Current security model

### Root credentials

The standalone `byoridb-server` requires a nonblank
`BYORIDB_ROOT_PASSWORD` and refuses to start without it. The value is not
written to logs. Store it in a deployment secret manager and do not commit it
to source control, a container image, or an `.env` file.

The `root` identity is reserved and its password cannot be changed with
`CREATE USER`, `ALTER USER`, or `DROP USER`. Rotate it by changing
`BYORIDB_ROOT_PASSWORD` and restarting the server. A restart invalidates all
live sessions in that process.

User passwords are stored as Argon2 password hashes. Empty and whitespace-only
passwords are rejected. Authentication failures use a generic external error
so callers cannot directly distinguish an unknown, disabled, or wrong-password
account.

### Authorization

ByoriDB enforces statement-level role checks. User and role administration,
`SHOW USER`, `SHOW SESSIONS`, and `BALANCE` require `GOD` or `ADMIN`; nested
compound and profiled statements are checked recursively. The current `SHOW
USER` result is a built-in-root placeholder. `SHOW SESSIONS` lists active users
and selected spaces but omits bearer session IDs. The public parser does not
accept `SHOW USERS` or `SHOW ROLES`.

The built-in roles are `GOD`, `ADMIN`, `DBA`, `USER`, and `GUEST`. Their current
permission entries target the wildcard space `*`. Although the internal model
can represent a space, the public query language does not currently provide a
space-scoped `GRANT` operation. The built-in RBAC model is therefore not a
multi-tenant or per-space isolation boundary.

### Sessions

Session IDs are random positive 63-bit values and must be treated as bearer
credentials. The default session lifetime is 24 hours. Authentication/session
state is held in memory and shared by HTTP and gRPC only within the same server
process.

Changing a user's password or roles, disabling the user, or dropping the user
invalidates that user's sessions in the local process. Session creation and
revocation are not coordinated across separate ByoriDB processes. A process
restart invalidates all of its sessions.

Do not put session IDs in application logs, analytics, traces, or error reports.
The HTTP sign-out endpoint currently carries the session ID in the URL path, so
reverse-proxy access logs must redact or exclude that route.

### Diagnostics and logs

`GET /api/v1/diagnostics/queries` requires a live `GOD` or `ADMIN` session in
the `Authorization: Bearer <session-id>` header. Its response omits raw session
IDs and redacts query text after a `PASSWORD` keyword. Authentication and
invalid-session responses do not intentionally reflect credentials.

These safeguards do not control logs added by a reverse proxy, service mesh,
client, or operator. Configure those systems separately. `/health`, `/ready`,
`/metrics`, and `/api/v1/metrics` are currently unauthenticated; restrict them
at the network or proxy layer if their availability or operational metadata is
sensitive.

## Known deployment boundaries

- **No native transport encryption:** HTTP and gRPC are plaintext. Terminate TLS
  with a trusted ingress, proxy, or service mesh and authenticate the hop to the
  backend where required.
- **No general login rate limiter:** in-process failed-attempt tracking is not a
  substitute for per-source throttling. Apply connection limits and login rate
  limits at the network edge.
- **No per-space grant surface:** built-in roles apply across all spaces. Use
  separate trusted deployments when strong tenant isolation is required.
- **Process-local sessions:** revocation and expiry are not distributed between
  instances. Do not horizontally scale the current standalone Graph service as
  if it had a shared session authority.
- **Incomplete distributed operation:** cluster configuration and custom Raft
  components exist, but storage peer bootstrap, deployment wiring, and
  multi-node operational validation are incomplete. Distributed mode is not a
  production security or availability boundary.
- **Public operational endpoints:** health, readiness, and metrics endpoints do
  not require a session.
- **Relaxed durability can lose data:** `BYORIDB_DURABILITY=relaxed`, `none`, or
  `eventual` disables per-commit fsync. Use it only for reloadable bulk imports,
  not steady-state serving.

## Deployment checklist

- Run a reviewed, up-to-date revision and monitor dependency advisories.
- Inject a unique, high-entropy `BYORIDB_ROOT_PASSWORD` from a secret manager.
- Bind listeners to loopback or private interfaces when possible; firewall both
  default ports.
- Terminate TLS and apply authentication, request-size controls, and rate limits
  at the edge.
- Restrict unauthenticated health and metrics routes to trusted monitoring
  systems.
- Do not rely on built-in roles to isolate mutually untrusted tenants or spaces.
- Keep one trusted standalone server process unless you have independently
  designed and validated multi-instance session and data coordination.
- Redact session-bearing paths, authorization headers, and password-bearing
  query bodies from every external logging layer.
- Protect the data directory and backups with least-privilege filesystem access;
  backups contain both current data and temporal history.
- Keep immediate durability for serving workloads, test backup restoration, and
  verify the restored database before replacing active data.
- Rotate the root secret with an environment change and restart; expect all
  sessions to reconnect.

## Scope

This policy covers code maintained in the
[byoridb/byoridb](https://github.com/byoridb/byoridb) repository. Vulnerabilities
in an upstream dependency should also be reported to that upstream project when
appropriate. Reports about the separate agent-memory product belong in the
[byoridb/byori](https://github.com/byoridb/byori) repository.

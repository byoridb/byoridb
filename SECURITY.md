# Security policy

[한국어](SECURITY.ko.md)

## Report a vulnerability privately

Do not disclose a suspected vulnerability in a public issue, discussion, pull
request, log excerpt, or chat transcript.

Use GitHub's private vulnerability reporting flow from the repository's
**Security** tab, or open a
[private security advisory report](https://github.com/byoridb/byoridb/security/advisories/new).
Include only the information needed to reproduce and assess the issue:

- the affected revision or release;
- the deployment mode and operating system;
- the affected interface (HTTP, gRPC, CLI, storage, backup, or cluster);
- prerequisites and a minimal reproduction;
- expected and observed behavior; and
- the potential confidentiality, integrity, or availability impact.

Remove real credentials and private data. If GitHub does not offer a private
reporting control, open a public issue with no vulnerability details and ask a
maintainer for a private channel.

Maintainers will validate the report, assess affected versions, and coordinate
a fix and disclosure when appropriate. This project does not currently promise
a fixed security-response SLA.

## Supported versions

| Version | Security support |
|---|---|
| Reviewed current `main` revision | Target for security fixes |
| Tagged releases and older revisions | No maintained backport branch or support promise |

Release tags are immutable snapshots, not maintained support lines. Do not
assume that a fix merged to `main` has been backported to an existing tag or
binary.

## Current security model

### Credentials and authentication

The standalone `byoridb-server` requires a non-empty
`BYORIDB_ROOT_PASSWORD` and refuses to start without it. Keep this value in a
deployment secret manager; do not commit it to source control, a container
image, or an `.env` file.

The `root` identity is reserved. `CREATE USER`, `ALTER USER`, and `DROP USER`
cannot replace or modify it. Rotate the root credential by changing
`BYORIDB_ROOT_PASSWORD` and restarting the server. A restart invalidates all
sessions held by that process.

Persisted user passwords are stored as salted Argon2 hashes. HTTP and gRPC
authentication failures return a generic external error so callers cannot
directly distinguish an unknown, disabled, locked, or wrong-password account.

### Authorization

ByoriDB enforces statement-level role checks. User and role administration,
`SHOW USERS`, `SHOW ROLES`, `SHOW SESSIONS`, and `BALANCE` require `GOD` or
`ADMIN`. Compound statements and the inner statement executed by `PROFILE` are
checked recursively.

`SHOW USERS` returns the built-in root account and persisted users without
password hashes. `SHOW SESSIONS` returns usernames and selected spaces but
omits bearer session IDs.

The built-in roles are `GOD`, `ADMIN`, `DBA`, `USER`, and `GUEST`. Their current
permissions use the wildcard space `*`; the query language does not provide a
space-scoped `GRANT`. Built-in RBAC is therefore not a per-space tenant
isolation boundary.

### Sessions and diagnostics

Session IDs are cryptographically random positive 63-bit bearer credentials.
The default sliding session lifetime is 24 hours. In the standalone server,
HTTP and gRPC share authentication and session state within one process.
Password or role changes and user deletion revoke that user's local sessions.
Session creation and revocation are not coordinated across separate processes.

Do not put session IDs in logs, analytics, traces, or issue reports. HTTP
operations that use the `X-ByoriDB-Session-Id` header must be protected like
any other bearer-token flow.

`GET /api/v1/diagnostics/queries` requires a live `GOD` or `ADMIN` session in
that header. Its response includes only safe query metadata and omits raw query
text and session IDs. `/health`, `/ready`, `/metrics`, and `/api/v1/metrics`
are unauthenticated; restrict them at the network or proxy layer when their
availability or operational metadata is sensitive.

## Known deployment boundaries

- **No native transport encryption:** HTTP and gRPC are plaintext. Terminate
  TLS at a trusted ingress, proxy, or service mesh and protect the backend hop.
- **No general login rate limiter:** in-process account lockout is not a
  substitute for per-source throttling. Apply connection and login limits at
  the network edge.
- **No per-space grant surface:** use separate trusted deployments when strong
  isolation between mutually untrusted tenants is required.
- **Process-local sessions:** do not horizontally scale the standalone Graph
  service as though it had a shared session authority.
- **Incomplete distributed operation:** cluster and Raft components exist, but
  storage peer bootstrap, deployment wiring, and multi-node operational
  validation remain incomplete.
- **Relaxed durability can lose data:** `BYORIDB_DURABILITY=relaxed`, `none`, or
  `eventual` disables per-commit fsync. Use it only for reloadable bulk imports.

## Deployment checklist

- Run a reviewed, up-to-date revision and monitor dependency advisories.
- Inject a unique, high-entropy `BYORIDB_ROOT_PASSWORD` from a secret manager.
- Bind listeners to private interfaces where possible and firewall both APIs.
- Terminate TLS and apply request-size controls and rate limits at the edge.
- Restrict unauthenticated health, readiness, and metrics routes.
- Do not rely on built-in roles to isolate mutually untrusted tenants.
- Redact session headers, authentication responses, and password-bearing query
  bodies from every external logging layer.
- Protect the data directory and backups with least-privilege filesystem
  access; backups contain current data and temporal history.
- Keep immediate durability for serving workloads and test backup restoration.
- Rotate the root secret by changing the environment and restarting; expect all
  clients to reconnect.

## Scope

This policy covers code maintained in the
[byoridb/byoridb](https://github.com/byoridb/byoridb) repository. Reports about
the separate agent-memory product belong in the
[byoridb/byori](https://github.com/byoridb/byori) repository.

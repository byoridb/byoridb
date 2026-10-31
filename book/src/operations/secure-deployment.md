# TLS single-node deployment

ByoriDB's native listeners use plaintext HTTP and gRPC. The standalone [`deploy/secure/compose.yml`](https://github.com/byoridb/byoridb/blob/main/deploy/secure/compose.yml) profile terminates TLS at NGINX and connects to a single database on a private Docker network. Only the proxy publishes ports. This profile does not provide replication, failover, native TLS, or encryption at rest.

Both running services use UID/GID `10001:10001`, drop Linux capabilities, and use read-only root filesystems. Database writes go to a named volume owned by `10001`; temporary files use bounded tmpfs mounts. Existing bind mounts or volumes created by root must be migrated to that owner before starting this image. Stop the database and take a verified backup before changing existing storage permissions.

The source image builds the server, backup tool, bulk loader and CLI with `cargo build --locked`. Rust, runtime and NGINX images are pinned to registry digests; update them deliberately and rerun the deployment smoke. OS package updates are resolved from the distribution's repositories during each build. The prebuilt Dockerfile requires matching Linux architecture and a glibc version compatible with Ubuntu 24.04.

## Start the profile

Use a certificate whose SAN covers the hostname clients will use. Supply the full certificate chain and its matching private key from your certificate authority. Keep source credentials outside the repository. The installer creates versioned certificate/key pairs with mode `0640` in a directory with mode `0750`; the proxy reads them through a supplementary group.

From the repository root:

```bash
export BYORIDB_ROOT_PASSWORD="$(openssl rand -base64 36)"
export BYORIDB_TLS_CERT_DIR="$HOME/.local/share/byoridb/tls"
export BYORIDB_TLS_CERT_GID="$(id -g)"
python3 deploy/secure/rotate_cert.py \
  --cert-dir "$BYORIDB_TLS_CERT_DIR" \
  --cert /secure-input/fullchain.pem --key /secure-input/privkey.pem \
  --no-reload

docker compose -f deploy/secure/compose.yml --project-name byoridb-secure \
  up --detach --build --wait
```

Store the generated root password in your deployment secret manager; use the same value on subsequent starts. Docker host administrators can inspect container environment variables, so the Docker host remains a trusted boundary. For a dedicated certificate group, create the certificate directory with that group and set `BYORIDB_TLS_CERT_GID` to its numeric ID. The account running the installer must be able to write to that directory and assign its group.

The defaults bind HTTPS to `127.0.0.1:8443` and gRPC over TLS to `127.0.0.1:9443`. Set `BYORIDB_TLS_BIND` to the intended interface and restrict that interface with the host/network firewall when enabling network clients. `BYORIDB_HTTPS_PORT` and `BYORIDB_GRPCS_PORT` change the host ports. Do not combine this profile with the repository's local `docker-compose.yml`, which publishes plaintext loopback listeners for local development.

Use clients that verify the server hostname and trust its certificate authority. The bundled CLI currently connects to the private plaintext backend; run it inside the server container for administration. Do not assume that passing an `https://` address adds TLS support to that CLI.

## Private operations and logs

The proxy allows only `/api/v1/session`, `/api/v1/discover`, `/api/v1/query`, `/api/v1/query/json` and the five declared `graph.GraphService` RPC methods. It returns `403` for `/health`, `/ready`, metrics, diagnostics, unknown HTTP routes and unknown RPC methods. Readiness remains available inside the server container:

```bash
docker compose -f deploy/secure/compose.yml --project-name byoridb-secure \
  exec -T server curl --fail --silent http://127.0.0.1:19669/ready
```

The database backend has no published host ports. TLS ends at the proxy; traffic between the two containers is plaintext on an internal Docker network. Do not attach untrusted containers to that network. The backend sees the proxy's peer IP, so its per-peer login throttle is shared by clients using this proxy; account throttles still apply separately.

NGINX access and error request logs are disabled because they can contain credential-bearing URLs. ByoriDB runs at `info` and records statement type, size and outcome rather than bodies or bearer session IDs. Do not add request-body/header logging or enable debug/trace logging in this profile. Use private readiness, process state and application metrics for operations.

## Rotate the certificate

Obtain a renewed certificate with a matching key and the same service hostname. Keep the previous certificate directories until new connections have been verified.

```bash
python3 deploy/secure/rotate_cert.py \
  --cert-dir "$BYORIDB_TLS_CERT_DIR" \
  --cert /secure-input/renewed-fullchain.pem --key /secure-input/renewed-privkey.pem \
  --project byoridb-secure
```

The installer checks that the key matches and the leaf certificate has not expired, atomically replaces `active.conf` with paths to a complete versioned certificate/key pair, tests the NGINX configuration, and requests a graceful reload. Validation or reload failure restores the previous configuration; retained certificate directories support manual recovery. Existing connections can continue with the old TLS session; open fresh HTTPS and gRPC connections to verify the new certificate. A CA change also requires updating client trust before the old CA is removed. If the database container is recreated with a different private IP, reload the proxy to refresh its upstream resolution.

## Reproducible deployment check

Docker Engine with Compose, Python 3, OpenSSL and curl with HTTP/2 support are required:

```bash
python3 deploy/secure/smoke.py --build
# Or validate an already-built source or prebuilt image:
python3 deploy/secure/smoke.py --image byoridb-server:local
```

The smoke creates its own random Compose project, ports, one-day test CA and leaf certificates. It checks nonroot execution, volume ownership, shipped binaries, private backend ports, denied operations routes, real HTTPS and gRPC authentication/query results, exact persisted results after a server restart, certificate trust, rotation and absence of credentials/query bodies in container logs. It removes only its own containers, network and data volume. Test certificates are never suitable for production. The test needs local disk space for a Linux release build and does not replace a backup recovery drill or capacity testing.

Configuration follows the official [NGINX HTTPS](https://nginx.org/en/docs/http/configuring_https_servers.html), [gRPC proxy](https://nginx.org/en/docs/http/ngx_http_grpc_module.html), [graceful reload](https://nginx.org/en/docs/control.html), and [Docker Compose service](https://docs.docker.com/reference/compose-file/services/) documentation.

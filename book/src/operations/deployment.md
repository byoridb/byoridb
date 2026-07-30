# Deployment

[한국어](../ko/operations/deployment.html)

The supported runtime shape is one `byoridb-server` process with one local redb
data directory. The server exposes Graph gRPC on port `9669` and HTTP on port
`19669` by default.

Do not deploy multiple independent processes as a shared cluster. The
distributed components are not fully wired into the launcher; see
[Distributed systems](../architecture/distributed.html).

## Required secret

The standalone server refuses to start unless `BYORIDB_ROOT_PASSWORD` is set to
a non-empty value:

```bash
export BYORIDB_ROOT_PASSWORD='replace-with-a-managed-secret'
```

Inject it from the environment's secret manager. Do not place it in an image,
ConfigMap, checked-in `.env` file, shell history, or command-line argument.
Root credentials are replaced only by changing this environment value and
restarting the server.

## Run from source

```bash
cargo build --locked --release -p byoridb --bin byoridb-server

export BYORIDB_ROOT_PASSWORD='replace-with-a-managed-secret'
export BYORIDB__STORAGE__DATA_PATHS=/var/lib/byoridb/data
./target/release/byoridb-server
```

`byoridb-server` does not accept a `--data-dir` flag. Configuration comes from
defaults, an optional `byoridb.toml` configuration file in the working
directory, and environment variables in the form `BYORIDB__SECTION__KEY`.

An equivalent minimal `byoridb.toml` is:

```toml
[server]
graph_addr = "0.0.0.0:9669"
http_addr = "0.0.0.0:19669"
storage_addr = "0.0.0.0:44500"

[storage]
data_paths = ["/var/lib/byoridb/data"]
```

Only the first `data_paths` entry is currently opened. Storage cache and
durability overrides are separate variables:

```bash
export BYORIDB_CACHE_SIZE_MB=4096
# Do not set BYORIDB_DURABILITY during normal serving.
```

Size the redb page cache and query memory guard from measured working-set and
query behavior; there is no universal CPU, memory, or disk recommendation.

## Docker

Build the image directly:

```bash
docker build -t byoridb-server:local .
docker run --rm \
  -e BYORIDB_ROOT_PASSWORD \
  -e BYORIDB__STORAGE__DATA_PATHS=/app/data \
  -p 9669:9669 \
  -p 19669:19669 \
  -v byoridb-data:/app/data \
  byoridb-server:local
```

Or run one service from the checked-in Compose file:

```bash
export BYORIDB_ROOT_PASSWORD='replace-with-a-managed-secret'
docker compose up --build byoridb-server-1
```

The three services in `docker-compose.yml` use separate named volumes and no
cluster settings. Starting all three creates three unrelated databases on
different host ports, not replicas.

## Repository AKS deployment

The Azure assets under `deploy/azure/` describe a single-node deployment:

- `bootstrap.sh` provisions Azure resources, builds an image, creates a root
  Secret if absent, restricts the public load balancer to an operator CIDR, and
  applies the manifests;
- `k8s/01-configmap.yaml` sets listener and data-path configuration;
- `k8s/03-statefulset.yaml` declares one replica, a ReadWriteOnce premium PVC,
  resource limits, graceful termination, and HTTP probes;
- `k8s/04-services.yaml` declares headless and public LoadBalancer Services;
- `.github/workflows/deploy.yml` substitutes a commit-tagged image before
  applying manifests and preserves the live load-balancer source ranges.

Read and adapt every value before running the bootstrap script. The checked-in
Service uses a documentation-only CIDR placeholder; it is not an allowlist for
your environment. Applying the raw StatefulSet can also reintroduce its
placeholder image instead of the intended commit image, so use the repository's
CI-gated rendering workflow.

The manifest's one replica and one PVC are intentional. Increasing the replica
count does not create a ByoriDB cluster.

These files show repository configuration, not the observed health, image, or
rollout status of a live AKS environment. Inspect the target environment at
deployment time.

## Health and shutdown

The Graph HTTP server exposes:

```bash
curl -f http://127.0.0.1:19669/health
curl -f http://127.0.0.1:19669/ready
```

- `/health` returns `OK` when the HTTP process can serve the handler.
- `/ready` returns `READY` while the service accepts new queries and changes to
  HTTP 503 once graceful shutdown begins.

There is no registered standard gRPC health service. Use the HTTP endpoints for
the checked-in Kubernetes probes.

On `SIGTERM` or Ctrl+C, the process fails readiness, waits up to 25 seconds for
in-flight queries, signals the network servers, and checkpoints redb. The AKS
manifest gives the pod a 300-second termination grace period so the process is
not killed during this sequence.

## Network security

ByoriDB currently serves plaintext HTTP and gRPC and has no native TLS
configuration. For any non-local deployment:

- terminate TLS at a trusted ingress, proxy, or load balancer;
- restrict both ports with private networking, firewall rules, security groups,
  or source ranges;
- keep `/metrics` and health endpoints off the public internet;
- add external request/rate limits appropriate to the environment;
- rotate and audit the secret source used for `BYORIDB_ROOT_PASSWORD`.

Authentication does not compensate for an exposed plaintext transport.

# 배포

[English](../../operations/deployment.html)

지원되는 runtime 형태는 하나의 local redb data directory를 사용하는 하나의
`byoridb-server` process입니다. 기본적으로 Graph gRPC는 `9669`, HTTP는 `19669`
port를 사용합니다.

여러 독립 process를 shared cluster로 배포하지 마세요. distributed 구성요소는 launcher에
완전히 연결되어 있지 않습니다. [분산 시스템](../architecture/distributed.html)을
확인하세요.

## 필수 secret

standalone server는 `BYORIDB_ROOT_PASSWORD`가 비어 있지 않은 값으로 설정되지 않으면
시작을 거부합니다.

```bash
export BYORIDB_ROOT_PASSWORD='replace-with-a-managed-secret'
```

환경의 secret manager에서 주입하세요. image, ConfigMap, commit되는 `.env`, shell
history, command-line argument에 넣지 마세요. root credential은 이 환경변수를 바꾸고
server를 restart해야만 교체됩니다.

## Source에서 실행

```bash
cargo build --release --bin byoridb-server

export BYORIDB_ROOT_PASSWORD='replace-with-a-managed-secret'
export BYORIDB__STORAGE__DATA_PATHS=/var/lib/byoridb/data
./target/release/byoridb-server
```

`byoridb-server`에는 `--data-dir` flag가 없습니다. 설정은 기본값, working directory의
선택적 `byoridb` config 파일, `BYORIDB__SECTION__KEY` 형식의 환경변수에서 옵니다.

동일한 최소 `byoridb.toml`은 다음과 같습니다.

```toml
[server]
graph_addr = "0.0.0.0:9669"
http_addr = "0.0.0.0:19669"
storage_addr = "0.0.0.0:44500"

[storage]
data_paths = ["/var/lib/byoridb/data"]
```

현재는 `data_paths`의 첫 번째 entry만 엽니다. storage cache와 durability override는
별도 환경변수입니다.

```bash
export BYORIDB_CACHE_SIZE_MB=4096
# 일반 serving에서는 BYORIDB_DURABILITY를 설정하지 마세요.
```

redb page cache와 query memory guard는 측정한 working set과 query 동작에 맞춰
결정하세요. 모든 환경에 적용되는 CPU, memory, disk 권장값은 없습니다.

## Docker

image를 직접 빌드합니다.

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

또는 저장소의 Compose 파일에서 service 하나를 실행합니다.

```bash
export BYORIDB_ROOT_PASSWORD='replace-with-a-managed-secret'
docker compose up --build byoridb-server-1
```

`docker-compose.yml`의 세 service는 각자 named volume을 사용하고 cluster 설정이
없습니다. 모두 시작하면 서로 다른 host port의 관계없는 database 세 개가 만들어지며
replica가 아닙니다.

## 저장소 AKS 배포

`deploy/azure/` 아래 Azure 자산은 single-node 배포를 설명합니다.

- `bootstrap.sh`는 Azure resource를 provision하고 image를 빌드하며, 없을 때 root Secret을
  만들고 public load balancer를 operator CIDR로 제한한 뒤 manifest를 적용합니다.
- `k8s/01-configmap.yaml`은 listener와 data path를 설정합니다.
- `k8s/03-statefulset.yaml`은 replica 하나, ReadWriteOnce premium PVC, resource limit,
  graceful termination, HTTP probe를 선언합니다.
- `k8s/04-services.yaml`은 headless와 public LoadBalancer Service를 선언합니다.
- `.github/workflows/deploy.yml`은 manifest 적용 전에 commit-tagged image를 치환하고 live
  load-balancer source range를 보존합니다.

bootstrap script 실행 전에 모든 값을 읽고 환경에 맞게 바꾸세요. commit된 Service의
CIDR은 문서용 placeholder이며 실제 환경의 allowlist가 아닙니다. raw StatefulSet을
적용하면 의도한 commit image 대신 placeholder image가 다시 들어갈 수 있으므로 지원되는
rendering workflow를 사용하세요.

manifest의 replica 하나와 PVC 하나는 의도된 구성입니다. replica 수를 늘려도 ByoriDB
cluster가 만들어지지 않습니다.

이 파일은 저장소 설정을 보여줄 뿐 live AKS 환경의 현재 health, image, rollout status를
보여주지 않습니다. 배포 시 target 환경을 직접 검사하세요.

## Health와 shutdown

Graph HTTP server는 다음 endpoint를 제공합니다.

```bash
curl -f http://127.0.0.1:19669/health
curl -f http://127.0.0.1:19669/ready
```

- `/health`는 HTTP process가 handler를 제공할 수 있을 때 `OK`를 반환합니다.
- `/ready`는 service가 새 query를 받을 때 `READY`를 반환하고 graceful shutdown이
  시작되면 HTTP 503으로 바뀝니다.

등록된 표준 gRPC health service는 없습니다. 저장소의 Kubernetes probe에는 HTTP
endpoint를 사용하세요.

`SIGTERM` 또는 Ctrl+C를 받으면 process는 readiness를 실패시키고 최대 25초 동안
in-flight query를 기다린 뒤 network server에 종료를 알리고 redb를 checkpoint합니다.
AKS manifest는 이 과정 중 강제 종료되지 않도록 pod에 300초 termination grace period를
줍니다.

## Network security

현재 ByoriDB는 plaintext HTTP/gRPC를 제공하며 native TLS 설정이 없습니다. local이 아닌
배포에서는 다음을 적용하세요.

- 신뢰할 수 있는 ingress, proxy, load balancer에서 TLS termination
- private network, firewall, security group, source range로 두 port 제한
- `/metrics`와 health endpoint를 public internet에서 차단
- 환경에 맞는 external request/rate limit 추가
- `BYORIDB_ROOT_PASSWORD`를 공급하는 secret source rotation과 audit

인증은 노출된 plaintext transport를 보완하지 못합니다.

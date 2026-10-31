# 배포

[English](../../operations/deployment.html) | **한국어**

지원되는 runtime은 local redb data directory 하나를 사용하는 `byoridb-server`
단일 process입니다. 기본 port는 Graph gRPC `9669`, HTTP `19669`입니다.

여러 독립 process를 데이터를 공유하는 cluster로 배포하면 안 됩니다. 분산 component는
launcher에 완전히 연결되지 않았으며 기본값이 아닌 cluster 설정은 시작 전에 거부합니다.
[분산 시스템](../architecture/distributed.html)을 참고하세요.

## 필수 secret

Standalone server는 `BYORIDB_ROOT_PASSWORD`가 없거나 비어 있으면 시작을 거부합니다.

```bash
export BYORIDB_ROOT_PASSWORD='replace-with-a-managed-secret'
```

운영 환경의 secret manager에서 주입하세요. Image, ConfigMap, 저장소에 넣는 `.env`,
shell history, command-line 인자에 담지 마세요. Root credential은 이 환경변수를
변경하고 서버를 재시작해야 교체됩니다.

## 소스에서 실행

```bash
cargo build --locked --release -p byoridb --bin byoridb-server

export BYORIDB_ROOT_PASSWORD='replace-with-a-managed-secret'
export BYORIDB__STORAGE__DATA_PATHS=/var/lib/byoridb/data
./target/release/byoridb-server
```

`byoridb-server`는 `--data-dir` flag를 받지 않습니다. 기본값, 작업 디렉터리의 선택적
`byoridb.toml`, `BYORIDB__SECTION__KEY` 환경변수 순으로 설정을 적용합니다.

같은 의미의 최소 `byoridb.toml`은 다음과 같습니다.

```toml
[server]
graph_addr = "0.0.0.0:9669"
http_addr = "0.0.0.0:19669"
storage_addr = "0.0.0.0:44500"

[storage]
data_paths = ["/var/lib/byoridb/data"]
```

현재 `data_paths`의 첫 항목만 엽니다. Storage cache와 durability는 별도 변수입니다.

```bash
export BYORIDB_CACHE_SIZE_MB=4096
# Do not set BYORIDB_DURABILITY during normal serving.
```

실제 working set과 query 동작을 측정해 redb page cache와 query memory guard를
정하세요. 공통으로 적용할 CPU·메모리·디스크 권장값은 없습니다.

## Docker

Image를 직접 빌드할 수 있습니다.

```bash
docker build -t byoridb-server:local .
docker run --rm \
  -e BYORIDB_ROOT_PASSWORD \
  -e BYORIDB__STORAGE__DATA_PATHS=/app/data \
  -p 127.0.0.1:9669:9669 \
  -p 127.0.0.1:19669:19669 \
  -v byoridb-data:/app/data \
  byoridb-server:local
```

또는 저장소의 Compose 파일로 서비스 하나를 실행합니다.

```bash
export BYORIDB_ROOT_PASSWORD='replace-with-a-managed-secret'
docker compose up --build byoridb
```

기본 Compose는 loopback 전용 host port와 영속 named volume을 사용하는 standalone
서버 하나를 시작합니다. 두 image는 UID/GID 10001로 실행하고 offline backup 도구를
포함합니다. 기존 bind mount 데이터는 이 사용자가 소유해야 합니다. 관련 없는 경로의
소유권을 재귀적으로 바꾸지 마세요.

네트워크 접근에는 검증된 [TLS 프로필](secure-deployment.md)을 사용하세요. 별도 TLS
HTTP/gRPC port, 비공개 backend, 운영 endpoint를 제외한 허용 목록과 인증서 교체
절차를 제공합니다.

## 저장소의 AKS 배포

`deploy/azure/`의 Azure asset은 단일 노드 배포를 정의합니다.

- `bootstrap.sh`는 Azure resource와 image를 만들고, root Secret이 없으면 생성하며,
  공개 load balancer를 운영자 CIDR로 제한한 뒤 manifest를 적용합니다.
- `k8s/01-configmap.yaml`은 listener와 data path를 설정합니다.
- `k8s/03-statefulset.yaml`은 replica 하나, ReadWriteOnce premium PVC, resource limit,
  정상 종료와 HTTP probe를 정의합니다.
- `k8s/04-services.yaml`은 headless 및 공개 LoadBalancer Service를 정의합니다.
- `.github/workflows/deploy.yml`은 commit tag image를 넣어 manifest를 적용하고 기존
  load-balancer source range를 보존합니다.

Bootstrap 전에 모든 값을 읽고 환경에 맞게 바꾸세요. 저장소의 Service는 문서용 CIDR
placeholder를 사용하므로 실제 환경의 허용 목록이 아닙니다. 원본 StatefulSet만 적용하면
원하는 commit image 대신 placeholder image가 다시 들어갈 수 있으므로 저장소의
CI-gated rendering workflow를 사용하세요.

기존 AKS manifest에는 TLS가 없습니다. Source-IP 허용 목록은 credential을 암호화하지
않으므로 신뢰할 수 없는 네트워크에 plaintext 서비스를 노출하지 마세요. 검증된 TLS
프로필을 지원 배포 기준으로 삼고, AKS 적용 전 동등한 ingress/private-backend 경계를
검증하세요.

Replica 하나와 PVC 하나는 의도된 설정입니다. Replica 수를 늘려도 ByoriDB cluster가
생기지 않습니다.

Pod는 비root image에 맞춰 UID/GID와 `fsGroup` 10001을 사용합니다. 기존 root 소유
배포를 업그레이드하기 전에 복원한 PVC에서 storage driver의 group 소유권 지원을
검증하세요. 큰 volume의 소유권 이전은 시간이 걸릴 수 있습니다.

이 파일은 저장소의 설정이며 실제 AKS의 health·image·rollout 상태를 보고하지 않습니다.
배포할 때 대상 환경을 직접 확인하세요.

## 상태 확인과 종료

Graph HTTP server는 다음 endpoint를 제공합니다.

```bash
curl -f http://127.0.0.1:19669/health
curl -f http://127.0.0.1:19669/ready
```

- `/health`는 HTTP process가 handler를 처리할 수 있으면 `OK`를 반환합니다.
- `/ready`는 새 query를 받는 동안 `READY`를 반환하며 정상 종료가 시작되면 HTTP 503으로
  바뀝니다.

표준 gRPC health service는 등록돼 있지 않습니다. 저장소의 Kubernetes probe에는
HTTP endpoint를 사용하세요.

서버는 storage를 열거나 readiness를 알리기 전에 두 API listener를 모두 확보합니다.
둘 중 하나의 bind 실패나 운영 중 예기치 않은 종료는 process 실패로 처리합니다.
`SIGTERM` 또는 Ctrl+C를 받으면 readiness를 내리고 최대 25초 동안 in-flight 요청을
기다린 뒤 network server를 종료하고 redb checkpoint를 수행합니다. Drain timeout이나
checkpoint 실패는 nonzero exit로 보고하며, 종료 전 write 결과가 불확실할 수 있습니다.
AKS manifest는 이 순서 도중 강제 종료되지 않도록 300초 termination grace period를 둡니다.

## 네트워크 보안

ByoriDB 자체 HTTP/gRPC는 plaintext이며 native TLS 설정이 없습니다. 비로컬 배포에서는:

- 신뢰할 수 있는 ingress·proxy·load balancer에서 TLS를 종료하세요.
- Private network, firewall, security group 또는 source range로 두 port를 제한하세요.
- `/metrics`와 health endpoint를 공개 internet에서 차단하세요.
- 환경에 맞는 외부 request/rate limit을 적용하세요.
- `BYORIDB_ROOT_PASSWORD`의 secret source를 교체하고 감사하세요.

인증만으로 노출된 plaintext 전송의 위험을 해소할 수는 없습니다.

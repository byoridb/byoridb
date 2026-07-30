# 배포

프로덕션 환경에 ByoriDB를 배포하기 위한 가이드입니다.

## 배포 모드

### 단독(Standalone) 모드

모든 서비스가 단일 프로세스에서 실행됩니다. 현재 지원되는 운영 경로입니다:

```bash
BYORIDB_ROOT_PASSWORD='<secret>' \
BYORIDB__STORAGE__DATA_PATHS=/var/lib/byoridb \
  byoridb-server
```

### 분산(Distributed) 모드

Raft, partition allocator와 분산 조회 구성요소는 있지만 Storage/Raft bootstrap을 포함한
multi-node launcher와 배포 wiring은 완성되지 않았습니다. 별도 `byoridb-meta`,
`byoridb-storage`, `byoridb` 실행 파일로 클러스터를 구성하는 명령은 제공하지 않습니다.

## 하드웨어 권장 사양

아래 수치는 초기 capacity-planning 예시이며 검증된 SLO나 multi-node 권장 구성은
아닙니다. standalone에서는 세 역할의 합산 메모리와 실제 dataset working set을 기준으로
부하 테스트하세요.

### Meta Service

| 구성 요소 | 최소 | 권장 |
|-----------|---------|-------------|
| CPU | 2 cores | 4 cores |
| Memory | 4 GB | 8 GB |
| Disk | 10 GB SSD | 50 GB SSD |

### Storage Service

| 구성 요소 | 최소 | 권장 |
|-----------|---------|-------------|
| CPU | 4 cores | 8+ cores |
| Memory | 8 GB | 32+ GB |
| Disk | 100 GB SSD | NVMe SSD |

### Graph Service

| 구성 요소 | 최소 | 권장 |
|-----------|---------|-------------|
| CPU | 4 cores | 8+ cores |
| Memory | 4 GB | 16 GB |
| Disk | 최소 | 최소 |

## Docker 배포

### 단일 컨테이너

```bash
docker build -t byoridb-server:local .
docker run --name byoridb-server \
  -p 9669:9669 -p 19669:19669 \
  -e BYORIDB_ROOT_PASSWORD='<secret>' \
  -e BYORIDB__STORAGE__DATA_PATHS=/app/data \
  -v byoridb-data:/app/data \
  byoridb-server:local
```

저장소의 현재 `docker-compose.yml`은 서로 복제하지 않는 독립 standalone 인스턴스 3개를
띄우는 개발용 파일입니다. 클러스터나 고가용성 구성으로 해석하면 안 됩니다.
root 비밀번호의 checked-in 기본값은 두지 않습니다. 실행 전에 secret을 환경변수로
주입해야 하며, 누락하거나 빈 값이면 Compose가 시작을 거부합니다.

```bash
export BYORIDB_ROOT_PASSWORD='<secret>'
docker compose up --build
```

## Kubernetes 배포

`deploy/azure/k8s`의 manifest는 `replicas: 1`인 standalone StatefulSet과 PVC를
배포합니다. image tag는 CI-gated deploy workflow가 성공한 commit SHA로 치환하므로 raw
StatefulSet 파일만 직접 apply하지 마세요. `byoridb-root` Secret에 비어 있지 않은
`BYORIDB_ROOT_PASSWORD`가 필요합니다.

## 프로덕션 설정

지원되는 standalone 설정 예시입니다.

```toml
[server]
graph_addr = "0.0.0.0:9669"
http_addr = "0.0.0.0:19669"

[storage]
data_paths = ["/var/lib/byoridb"]
```

## 보안

### TLS 경계

ByoriDB server에는 아직 native TLS 설정이 없습니다. 외부에 노출할 때는 ingress/reverse
proxy나 service mesh에서 TLS를 종료하고, server port는 private network와 방화벽으로
제한하세요. native TLS가 있는 것으로 가정한 `byoridb.toml` 설정은 동작하지 않습니다.

### 인증

시작 전에 root 비밀번호를 설정하고 배포 시크릿 매니저에 저장하세요:

```bash
export BYORIDB_ROOT_PASSWORD='strong-password'
```

`root`로 접속한 후 nGQL로 애플리케이션 사용자를 생성하세요.

## 헬스 체크

### HTTP 헬스 엔드포인트

```bash
curl http://localhost:19669/health
```

tonic 표준 gRPC health service는 아직 등록돼 있지 않으므로 `grpc_health_probe`는 사용할
수 없습니다. liveness/readiness에는 HTTP `/health`와 `/ready`를 사용하세요.

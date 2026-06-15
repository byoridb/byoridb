# 배포

프로덕션 환경에 ByoriDB를 배포하기 위한 가이드입니다.

## 배포 모드

### 단독(Standalone) 모드

모든 서비스가 단일 프로세스에서 실행됩니다 — 개발 및 테스트에 적합합니다:

```bash
byoridb-server --data-dir /var/lib/byoridb
```

### 분산(Distributed) 모드

프로덕션 확장성을 위해 서비스를 분리합니다:

```bash
# Meta Service (1-3 nodes for HA)
byoridb-meta --config /etc/byoridb/meta.toml

# Storage Service (3+ nodes)
byoridb-storage --config /etc/byoridb/storage.toml

# Graph Service (2+ nodes, stateless)
byoridb --config /etc/byoridb/graph.toml
```

## 하드웨어 권장 사양

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

### Docker Compose

```yaml
version: '3.8'

services:
  meta:
    image: byoridb/meta:latest
    ports:
      - "9559:9559"
    volumes:
      - meta-data:/data

  storage:
    image: byoridb/storage:latest
    ports:
      - "9779:9779"
    volumes:
      - storage-data:/data
    depends_on:
      - meta

  graph:
    image: byoridb/graph:latest
    ports:
      - "9669:9669"
      - "19669:19669"
    depends_on:
      - meta
      - storage

volumes:
  meta-data:
  storage-data:
```

다음 명령으로 시작합니다:

```bash
docker-compose up -d
```

## Kubernetes 배포

### 기본 StatefulSet

```yaml
apiVersion: apps/v1
kind: StatefulSet
metadata:
  name: byoridb-storage
spec:
  serviceName: byoridb-storage
  replicas: 3
  selector:
    matchLabels:
      app: byoridb-storage
  template:
    metadata:
      labels:
        app: byoridb-storage
    spec:
      containers:
      - name: storage
        image: byoridb/storage:latest
        ports:
        - containerPort: 9779
        volumeMounts:
        - name: data
          mountPath: /data
  volumeClaimTemplates:
  - metadata:
      name: data
    spec:
      accessModes: ["ReadWriteOnce"]
      resources:
        requests:
          storage: 100Gi
```

## 프로덕션 설정

### Meta Service

```toml
[server]
bind_addr = "0.0.0.0:9559"
data_dir = "/var/lib/byoridb/meta"

[cluster]
peers = ["meta1:9559", "meta2:9559", "meta3:9559"]

[raft]
election_timeout_ms = 2000
heartbeat_interval_ms = 200
```

### Storage Service

```toml
[server]
bind_addr = "0.0.0.0:9779"
data_dir = "/var/lib/byoridb/storage"

[meta]
addrs = ["meta1:9559", "meta2:9559", "meta3:9559"]

[storage]
block_cache_size = "4GB"
write_buffer_size = "128MB"
```

### Graph Service

```toml
[server]
grpc_addr = "0.0.0.0:9669"
http_addr = "0.0.0.0:19669"

[meta]
addrs = ["meta1:9559", "meta2:9559", "meta3:9559"]

[storage]
addrs = ["storage1:9779", "storage2:9779", "storage3:9779"]
```

## 보안

### TLS 설정

```toml
[tls]
enabled = true
cert_file = "/etc/byoridb/server.crt"
key_file = "/etc/byoridb/server.key"
ca_file = "/etc/byoridb/ca.crt"
```

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

### gRPC 헬스 체크

```bash
grpc_health_probe -addr=localhost:9669
```

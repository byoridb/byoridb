# Deployment

Guide for deploying ByoriDB in production environments.

## Deployment Modes

### Standalone Mode

All services in a single process - suitable for development and testing:

```bash
byoridb-server --data-dir /var/lib/byoridb
```

### Distributed Mode

Separate services for production scalability:

```bash
# Meta Service (1-3 nodes for HA)
byoridb-meta --config /etc/byoridb/meta.toml

# Storage Service (3+ nodes)
byoridb-storage --config /etc/byoridb/storage.toml

# Graph Service (2+ nodes, stateless)
byoridb --config /etc/byoridb/graph.toml
```

## Hardware Recommendations

### Meta Service

| Component | Minimum | Recommended |
|-----------|---------|-------------|
| CPU | 2 cores | 4 cores |
| Memory | 4 GB | 8 GB |
| Disk | 10 GB SSD | 50 GB SSD |

### Storage Service

| Component | Minimum | Recommended |
|-----------|---------|-------------|
| CPU | 4 cores | 8+ cores |
| Memory | 8 GB | 32+ GB |
| Disk | 100 GB SSD | NVMe SSD |

### Graph Service

| Component | Minimum | Recommended |
|-----------|---------|-------------|
| CPU | 4 cores | 8+ cores |
| Memory | 4 GB | 16 GB |
| Disk | Minimal | Minimal |

## Docker Deployment

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

Start with:

```bash
docker-compose up -d
```

## Kubernetes Deployment

### Basic StatefulSet

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

## Configuration for Production

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

## Security

### TLS Configuration

```toml
[tls]
enabled = true
cert_file = "/etc/byoridb/server.crt"
key_file = "/etc/byoridb/server.key"
ca_file = "/etc/byoridb/ca.crt"
```

### Authentication

Set the root password before startup and store it in your deployment secret
manager:

```bash
export BYORIDB_ROOT_PASSWORD='strong-password'
```

Create application users with nGQL after connecting as `root`.

## Health Checks

### HTTP Health Endpoint

```bash
curl http://localhost:19669/health
```

### gRPC Health Check

```bash
grpc_health_probe -addr=localhost:9669
```

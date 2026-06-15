# 백업 및 복원

정기적인 백업으로 데이터를 보호하세요.

## 백업 방법

### 스냅샷 백업

일관된 특정 시점(point-in-time) 스냅샷을 생성합니다:

```bash
# Create snapshot of all spaces
byoridb-admin backup snapshot --output /backup/snapshot_$(date +%Y%m%d)

# Create snapshot of specific space
byoridb-admin backup snapshot --space my_space --output /backup/my_space_backup
```

### 증분 백업

마지막 백업 이후 변경된 내용을 백업합니다:

```bash
# First full backup
byoridb-admin backup full --output /backup/full

# Subsequent incremental backups
byoridb-admin backup incremental --base /backup/full --output /backup/incr_1
```

### 연속 백업 (WAL 아카이빙)

특정 시점 복구를 위해 write-ahead log를 아카이빙합니다:

```toml
[backup]
wal_archive_enabled = true
wal_archive_path = "/backup/wal"
wal_archive_interval_secs = 60
```

## 백업 저장소

### 로컬 디스크

```bash
byoridb-admin backup snapshot --output /mnt/backup/byoridb
```

### 클라우드 스토리지 (S3)

```bash
byoridb-admin backup snapshot --output s3://bucket/byoridb/backup
```

필요한 환경변수:

```bash
export AWS_ACCESS_KEY_ID=your_key
export AWS_SECRET_ACCESS_KEY=your_secret
export AWS_REGION=us-west-2
```

## 복원

### 스냅샷에서 복원

```bash
# Stop the service first
systemctl stop byoridb

# Restore data
byoridb-admin restore --input /backup/snapshot_20240115 --data-dir /var/lib/byoridb

# Start service
systemctl start byoridb
```

### 특정 시점 복구 (Point-in-Time Recovery)

```bash
# Restore to specific point in time
byoridb-admin restore \
  --base /backup/full \
  --wal-dir /backup/wal \
  --target-time "2024-01-15T10:30:00Z" \
  --data-dir /var/lib/byoridb
```

### 특정 space 복원

```bash
byoridb-admin restore \
  --input /backup/snapshot \
  --space my_space \
  --data-dir /var/lib/byoridb
```

## 백업 스케줄

### Cron 사용

```bash
# /etc/cron.d/byoridb-backup

# Daily snapshot at 2 AM
0 2 * * * root /usr/bin/byoridb-admin backup snapshot --output /backup/daily/$(date +\%Y\%m\%d)

# Weekly full backup on Sunday
0 3 * * 0 root /usr/bin/byoridb-admin backup full --output /backup/weekly/$(date +\%Y\%m\%d)

# Clean up backups older than 30 days
0 4 * * * root find /backup/daily -mtime +30 -delete
```

### 백업 스크립트

```bash
#!/bin/bash
# backup.sh

BACKUP_DIR="/backup/byoridb"
DATE=$(date +%Y%m%d_%H%M%S)
RETENTION_DAYS=7

# Create backup
byoridb-admin backup snapshot --output "${BACKUP_DIR}/${DATE}"

# Verify backup
byoridb-admin backup verify --input "${BACKUP_DIR}/${DATE}"
if [ $? -ne 0 ]; then
    echo "Backup verification failed!" | mail -s "ByoriDB Backup Failed" admin@example.com
    exit 1
fi

# Cleanup old backups
find "${BACKUP_DIR}" -maxdepth 1 -mtime +${RETENTION_DAYS} -type d -exec rm -rf {} \;

echo "Backup completed: ${BACKUP_DIR}/${DATE}"
```

## 검증

항상 백업을 검증하세요:

```bash
# Verify backup integrity
byoridb-admin backup verify --input /backup/snapshot_20240115

# List contents
byoridb-admin backup list --input /backup/snapshot_20240115
```

## 모범 사례

1. **정기적인 테스트**: 주기적으로 테스트 환경에 백업을 복원해 보세요
2. **오프사이트 저장**: 백업을 주 데이터와 다른 위치에 보관하세요
3. **암호화**: 민감한 데이터가 포함된 백업은 암호화하세요
4. **모니터링**: 백업 실패 시 알림을 설정하세요
5. **문서화**: 복구 절차를 문서화하고 테스트하세요

## 재해 복구

### 복구 시간 목표 (RTO)

| 방법 | 일반적인 RTO |
|--------|-------------|
| 스냅샷 복원 | 수 분 |
| 특정 시점 복구 | 수 분에서 수 시간 |
| 전체 + 증분 | 수 시간 |

### 복구 시점 목표 (RPO)

| 방법 | 일반적인 RPO |
|--------|-------------|
| 연속 WAL 아카이브 | 수 초 |
| 시간별 스냅샷 | 1시간 |
| 일별 스냅샷 | 24시간 |

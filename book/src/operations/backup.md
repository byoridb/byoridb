# Backup & Restore

Protect your data with regular backups.

## Backup Methods

### Snapshot Backup

Create consistent point-in-time snapshots:

```bash
# Create snapshot of all spaces
byoridb-admin backup snapshot --output /backup/snapshot_$(date +%Y%m%d)

# Create snapshot of specific space
byoridb-admin backup snapshot --space my_space --output /backup/my_space_backup
```

### Incremental Backup

Back up changes since last backup:

```bash
# First full backup
byoridb-admin backup full --output /backup/full

# Subsequent incremental backups
byoridb-admin backup incremental --base /backup/full --output /backup/incr_1
```

### Continuous Backup (WAL Archiving)

Archive write-ahead logs for point-in-time recovery:

```toml
[backup]
wal_archive_enabled = true
wal_archive_path = "/backup/wal"
wal_archive_interval_secs = 60
```

## Backup Storage

### Local Disk

```bash
byoridb-admin backup snapshot --output /mnt/backup/byoridb
```

### Cloud Storage (S3)

```bash
byoridb-admin backup snapshot --output s3://bucket/byoridb/backup
```

Required environment variables:

```bash
export AWS_ACCESS_KEY_ID=your_key
export AWS_SECRET_ACCESS_KEY=your_secret
export AWS_REGION=us-west-2
```

## Restore

### From Snapshot

```bash
# Stop the service first
systemctl stop byoridb

# Restore data
byoridb-admin restore --input /backup/snapshot_20240115 --data-dir /var/lib/byoridb

# Start service
systemctl start byoridb
```

### Point-in-Time Recovery

```bash
# Restore to specific point in time
byoridb-admin restore \
  --base /backup/full \
  --wal-dir /backup/wal \
  --target-time "2024-01-15T10:30:00Z" \
  --data-dir /var/lib/byoridb
```

### Restore Specific Space

```bash
byoridb-admin restore \
  --input /backup/snapshot \
  --space my_space \
  --data-dir /var/lib/byoridb
```

## Backup Schedule

### Using Cron

```bash
# /etc/cron.d/byoridb-backup

# Daily snapshot at 2 AM
0 2 * * * root /usr/bin/byoridb-admin backup snapshot --output /backup/daily/$(date +\%Y\%m\%d)

# Weekly full backup on Sunday
0 3 * * 0 root /usr/bin/byoridb-admin backup full --output /backup/weekly/$(date +\%Y\%m\%d)

# Clean up backups older than 30 days
0 4 * * * root find /backup/daily -mtime +30 -delete
```

### Backup Script

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

## Verification

Always verify backups:

```bash
# Verify backup integrity
byoridb-admin backup verify --input /backup/snapshot_20240115

# List contents
byoridb-admin backup list --input /backup/snapshot_20240115
```

## Best Practices

1. **Regular Testing**: Periodically restore backups to a test environment
2. **Off-site Storage**: Keep backups in a different location than primary data
3. **Encryption**: Encrypt backups containing sensitive data
4. **Monitoring**: Alert on backup failures
5. **Documentation**: Document recovery procedures and test them

## Disaster Recovery

### Recovery Time Objective (RTO)

| Method | Typical RTO |
|--------|-------------|
| Snapshot restore | Minutes |
| Point-in-time recovery | Minutes to hours |
| Full + incremental | Hours |

### Recovery Point Objective (RPO)

| Method | Typical RPO |
|--------|-------------|
| Continuous WAL archive | Seconds |
| Hourly snapshots | 1 hour |
| Daily snapshots | 24 hours |

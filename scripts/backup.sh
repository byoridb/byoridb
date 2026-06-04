#!/bin/bash
# ByoriDB Automated Backup Script
#
# Usage: ./backup.sh [OPTIONS]
#
# This script creates a backup of the ByoriDB database and manages
# old backups according to the configured retention policy.
#
# Environment Variables:
#   BYORIDB_DATA_DIR     - Path to the database directory (default: /var/lib/byoridb/data)
#   BYORIDB_BACKUP_DIR   - Path to store backups (default: /var/lib/byoridb/backups)
#   BYORIDB_BACKUP_KEEP  - Number of backups to retain (default: 7)
#   BYORIDB_BACKUP_LABEL - Optional label for the backup
#   BYORIDB_BACKUP_BIN   - Path to the byoridb-backup binary
#
# Crontab Example (daily backup at 2 AM):
#   0 2 * * * /opt/byoridb/scripts/backup.sh >> /var/log/byoridb/backup.log 2>&1
#
# Crontab Example (hourly backup, keep 24):
#   0 * * * * BYORIDB_BACKUP_KEEP=24 /opt/byoridb/scripts/backup.sh >> /var/log/byoridb/backup.log 2>&1

set -e

# Configuration with defaults
BYORIDB_DATA_DIR="${BYORIDB_DATA_DIR:-/var/lib/byoridb/data}"
BYORIDB_BACKUP_DIR="${BYORIDB_BACKUP_DIR:-/var/lib/byoridb/backups}"
BYORIDB_BACKUP_KEEP="${BYORIDB_BACKUP_KEEP:-7}"
BYORIDB_BACKUP_LABEL="${BYORIDB_BACKUP_LABEL:-}"
BYORIDB_BACKUP_BIN="${BYORIDB_BACKUP_BIN:-byoridb-backup}"

# Timestamp for logging
timestamp() {
    date '+%Y-%m-%d %H:%M:%S'
}

log_info() {
    echo "[$(timestamp)] INFO: $1"
}

log_error() {
    echo "[$(timestamp)] ERROR: $1" >&2
}

log_warn() {
    echo "[$(timestamp)] WARN: $1"
}

# Check if byoridb-backup binary exists
if ! command -v "$BYORIDB_BACKUP_BIN" &> /dev/null; then
    # Try to find it in common locations
    if [ -x "/opt/byoridb/bin/byoridb-backup" ]; then
        BYORIDB_BACKUP_BIN="/opt/byoridb/bin/byoridb-backup"
    elif [ -x "./target/release/byoridb-backup" ]; then
        BYORIDB_BACKUP_BIN="./target/release/byoridb-backup"
    elif [ -x "./target/debug/byoridb-backup" ]; then
        BYORIDB_BACKUP_BIN="./target/debug/byoridb-backup"
    else
        log_error "byoridb-backup binary not found. Please set BYORIDB_BACKUP_BIN."
        exit 1
    fi
fi

# Validate data directory exists
if [ ! -d "$BYORIDB_DATA_DIR" ]; then
    log_error "Data directory does not exist: $BYORIDB_DATA_DIR"
    exit 1
fi

# Create backup directory if needed
if [ ! -d "$BYORIDB_BACKUP_DIR" ]; then
    log_info "Creating backup directory: $BYORIDB_BACKUP_DIR"
    mkdir -p "$BYORIDB_BACKUP_DIR"
fi

log_info "Starting ByoriDB backup..."
log_info "  Data directory: $BYORIDB_DATA_DIR"
log_info "  Backup directory: $BYORIDB_BACKUP_DIR"
log_info "  Retention count: $BYORIDB_BACKUP_KEEP"

# Build backup command
BACKUP_CMD="$BYORIDB_BACKUP_BIN create --db $BYORIDB_DATA_DIR --backup-dir $BYORIDB_BACKUP_DIR"

if [ -n "$BYORIDB_BACKUP_LABEL" ]; then
    BACKUP_CMD="$BACKUP_CMD --label \"$BYORIDB_BACKUP_LABEL\""
fi

# Create backup
log_info "Creating backup..."
if eval "$BACKUP_CMD"; then
    log_info "Backup created successfully"
else
    log_error "Backup creation failed"
    exit 1
fi

# Cleanup old backups
log_info "Cleaning up old backups (keeping $BYORIDB_BACKUP_KEEP)..."
if "$BYORIDB_BACKUP_BIN" cleanup --backup-dir "$BYORIDB_BACKUP_DIR" --keep "$BYORIDB_BACKUP_KEEP" --force; then
    log_info "Cleanup completed successfully"
else
    log_warn "Cleanup completed with warnings"
fi

# List current backups
log_info "Current backups:"
"$BYORIDB_BACKUP_BIN" list --backup-dir "$BYORIDB_BACKUP_DIR"

log_info "Backup process completed successfully"

// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Backup and restore functionality using RocksDB Checkpoint
//!
//! This module provides hot backup capabilities for ByoriDB's KVStore.
//! It uses RocksDB's Checkpoint feature to create consistent point-in-time
//! snapshots without blocking writes.
//!
//! # Usage
//!
//! ```ignore
//! use byoridb_kvstore::backup::{BackupManager, BackupOptions};
//!
//! let manager = BackupManager::new("/path/to/db", "/path/to/backups")?;
//!
//! // Create a backup
//! let backup_info = manager.create_backup(None)?;
//!
//! // List backups
//! let backups = manager.list_backups()?;
//!
//! // Restore from backup
//! manager.restore_backup(&backup_info.id, "/path/to/restore")?;
//! ```

use rocksdb::{checkpoint::Checkpoint, Options, DB};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tracing::{debug, info, warn};

/// Backup-related errors
#[derive(Debug, Error)]
pub enum BackupError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("RocksDB error: {0}")]
    RocksDB(#[from] rocksdb::Error),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Backup not found: {0}")]
    BackupNotFound(String),

    #[error("Invalid backup: {0}")]
    InvalidBackup(String),

    #[error("Restore target already exists: {0}")]
    RestoreTargetExists(String),
}

pub type Result<T> = std::result::Result<T, BackupError>;

/// Metadata for a backup
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupInfo {
    /// Unique backup identifier (timestamp-based)
    pub id: String,
    /// Timestamp when backup was created (Unix epoch seconds)
    pub created_at: u64,
    /// Size of backup in bytes
    pub size_bytes: u64,
    /// Optional description/label
    pub label: Option<String>,
    /// Source database path
    pub source_path: String,
}

/// Options for creating backups
#[derive(Debug, Clone)]
pub struct BackupOptions {
    /// Optional label for the backup
    pub label: Option<String>,
    /// Whether to flush WAL before backup (ensures all data is persisted)
    pub flush_before_backup: bool,
}

impl Default for BackupOptions {
    fn default() -> Self {
        Self {
            label: None,
            flush_before_backup: true,
        }
    }
}

/// Manages backups for a RocksDB database
pub struct BackupManager {
    /// Path to the source database
    db_path: PathBuf,
    /// Path to the backup directory
    backup_dir: PathBuf,
}

impl BackupManager {
    /// Create a new BackupManager
    ///
    /// # Arguments
    /// * `db_path` - Path to the RocksDB database directory
    /// * `backup_dir` - Path where backups will be stored
    pub fn new<P: AsRef<Path>, Q: AsRef<Path>>(db_path: P, backup_dir: Q) -> Result<Self> {
        let db_path = db_path.as_ref().to_path_buf();
        let backup_dir = backup_dir.as_ref().to_path_buf();

        // Create backup directory if it doesn't exist
        if !backup_dir.exists() {
            fs::create_dir_all(&backup_dir)?;
            // Restrict access to owner only — backup files contain raw DB data
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&backup_dir, fs::Permissions::from_mode(0o700))?;
            }
            info!("Created backup directory: {:?}", backup_dir);
        }

        Ok(Self {
            db_path,
            backup_dir,
        })
    }

    /// Create a backup of the database
    ///
    /// Uses RocksDB's Checkpoint feature for consistent hot backups.
    /// The backup is created in a subdirectory named with a timestamp.
    pub fn create_backup(&self, options: Option<BackupOptions>) -> Result<BackupInfo> {
        let options = options.unwrap_or_default();

        // Generate backup ID from timestamp
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let backup_id = format!("backup_{}", timestamp);
        let backup_path = self.backup_dir.join(&backup_id);

        info!(
            "Creating backup '{}' from {:?} to {:?}",
            backup_id, self.db_path, backup_path
        );

        // Open the database - read-write if flush is needed, otherwise read-only
        let mut db_opts = Options::default();
        db_opts.create_if_missing(false);

        if options.flush_before_backup {
            // Open in read-write mode to allow flush
            let db = DB::open(&db_opts, &self.db_path)?;
            debug!("Flushing WAL before backup");
            db.flush()?;

            // Create checkpoint
            let checkpoint = Checkpoint::new(&db)?;
            checkpoint.create_checkpoint(&backup_path)?;
        } else {
            // Open in read-only mode (safer, no modifications)
            let db = DB::open_for_read_only(&db_opts, &self.db_path, false)?;

            // Create checkpoint
            let checkpoint = Checkpoint::new(&db)?;
            checkpoint.create_checkpoint(&backup_path)?;
        }

        // Calculate backup size
        let size_bytes = calculate_dir_size(&backup_path)?;

        // Create backup info
        let backup_info = BackupInfo {
            id: backup_id,
            created_at: timestamp,
            size_bytes,
            label: options.label,
            source_path: self.db_path.to_string_lossy().to_string(),
        };

        // Save metadata
        self.save_backup_metadata(&backup_info)?;

        info!(
            "Backup '{}' created successfully ({} bytes)",
            backup_info.id, backup_info.size_bytes
        );

        Ok(backup_info)
    }

    /// List all available backups
    pub fn list_backups(&self) -> Result<Vec<BackupInfo>> {
        let mut backups = Vec::new();

        if !self.backup_dir.exists() {
            return Ok(backups);
        }

        for entry in fs::read_dir(&self.backup_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir()
                && path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("backup_"))
            {
                let metadata_path = path.join("backup_metadata.json");
                if metadata_path.exists() {
                    match self.load_backup_metadata(&path) {
                        Ok(info) => backups.push(info),
                        Err(e) => {
                            warn!("Failed to load backup metadata from {:?}: {}", path, e);
                        }
                    }
                } else {
                    // Try to reconstruct metadata from directory
                    if let Some(info) = self.reconstruct_backup_info(&path) {
                        backups.push(info);
                    }
                }
            }
        }

        // Sort by creation time (newest first)
        backups.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        Ok(backups)
    }

    /// Get information about a specific backup
    pub fn get_backup(&self, backup_id: &str) -> Result<BackupInfo> {
        let backup_path = self.backup_dir.join(backup_id);

        if !backup_path.exists() {
            return Err(BackupError::BackupNotFound(backup_id.to_string()));
        }

        self.load_backup_metadata(&backup_path).or_else(|_| {
            self.reconstruct_backup_info(&backup_path)
                .ok_or_else(|| BackupError::InvalidBackup(backup_id.to_string()))
        })
    }

    /// Restore a backup to a target directory
    ///
    /// # Arguments
    /// * `backup_id` - ID of the backup to restore
    /// * `target_path` - Path where the database will be restored
    /// * `overwrite` - If true, overwrite existing target directory
    pub fn restore_backup<P: AsRef<Path>>(
        &self,
        backup_id: &str,
        target_path: P,
        overwrite: bool,
    ) -> Result<()> {
        let backup_path = self.backup_dir.join(backup_id);
        let target_path = target_path.as_ref();

        if !backup_path.exists() {
            return Err(BackupError::BackupNotFound(backup_id.to_string()));
        }

        // Validate backup
        self.validate_backup(&backup_path)?;

        if target_path.exists() {
            if overwrite {
                warn!("Removing existing target directory: {:?}", target_path);
                fs::remove_dir_all(target_path)?;
            } else {
                return Err(BackupError::RestoreTargetExists(
                    target_path.to_string_lossy().to_string(),
                ));
            }
        }

        info!("Restoring backup '{}' to {:?}", backup_id, target_path);

        // Copy backup to target
        copy_dir_recursive(&backup_path, target_path)?;

        // Remove metadata file from restored directory (not needed in running DB)
        let metadata_in_target = target_path.join("backup_metadata.json");
        if metadata_in_target.exists() {
            fs::remove_file(metadata_in_target)?;
        }

        info!(
            "Backup '{}' restored successfully to {:?}",
            backup_id, target_path
        );

        Ok(())
    }

    /// Delete a backup
    pub fn delete_backup(&self, backup_id: &str) -> Result<()> {
        let backup_path = self.backup_dir.join(backup_id);

        if !backup_path.exists() {
            return Err(BackupError::BackupNotFound(backup_id.to_string()));
        }

        info!("Deleting backup '{}'", backup_id);
        fs::remove_dir_all(&backup_path)?;

        Ok(())
    }

    /// Delete old backups, keeping only the most recent N backups
    pub fn cleanup_old_backups(&self, keep_count: usize) -> Result<Vec<String>> {
        let backups = self.list_backups()?;
        let mut deleted = Vec::new();

        if backups.len() <= keep_count {
            return Ok(deleted);
        }

        // Delete older backups (list is sorted newest first)
        for backup in backups.into_iter().skip(keep_count) {
            info!("Cleaning up old backup: {}", backup.id);
            self.delete_backup(&backup.id)?;
            deleted.push(backup.id);
        }

        Ok(deleted)
    }

    /// Validate that a backup is complete and valid
    fn validate_backup(&self, backup_path: &Path) -> Result<()> {
        // Check for essential RocksDB files
        let current_file = backup_path.join("CURRENT");
        if !current_file.exists() {
            return Err(BackupError::InvalidBackup(
                "Missing CURRENT file".to_string(),
            ));
        }

        // Try to open the backup as read-only to verify integrity
        let mut opts = Options::default();
        opts.create_if_missing(false);

        match DB::open_for_read_only(&opts, backup_path, false) {
            Ok(_) => Ok(()),
            Err(e) => Err(BackupError::InvalidBackup(format!(
                "Failed to open backup: {}",
                e
            ))),
        }
    }

    /// Save backup metadata to JSON file
    fn save_backup_metadata(&self, info: &BackupInfo) -> Result<()> {
        let backup_path = self.backup_dir.join(&info.id);
        let metadata_path = backup_path.join("backup_metadata.json");

        let file = File::create(&metadata_path)?;
        let writer = BufWriter::new(file);

        serde_json::to_writer_pretty(writer, info)
            .map_err(|e| BackupError::Serialization(e.to_string()))?;

        Ok(())
    }

    /// Load backup metadata from JSON file
    fn load_backup_metadata(&self, backup_path: &Path) -> Result<BackupInfo> {
        let metadata_path = backup_path.join("backup_metadata.json");

        let file = File::open(&metadata_path)?;
        let reader = BufReader::new(file);

        serde_json::from_reader(reader).map_err(|e| BackupError::Serialization(e.to_string()))
    }

    /// Reconstruct backup info from directory when metadata is missing
    fn reconstruct_backup_info(&self, backup_path: &Path) -> Option<BackupInfo> {
        let id = backup_path.file_name()?.to_string_lossy().to_string();

        // Parse timestamp from ID (format: backup_<timestamp>)
        let timestamp: u64 = id.strip_prefix("backup_")?.parse().ok()?;

        let size_bytes = calculate_dir_size(backup_path).ok()?;

        Some(BackupInfo {
            id,
            created_at: timestamp,
            size_bytes,
            label: None,
            source_path: "unknown".to_string(),
        })
    }
}

/// Calculate total size of a directory recursively
fn calculate_dir_size(path: &Path) -> Result<u64> {
    let mut total = 0;

    if path.is_file() {
        return Ok(fs::metadata(path)?.len());
    }

    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_file() {
            total += fs::metadata(&path)?.len();
        } else if path.is_dir() {
            total += calculate_dir_size(&path)?;
        }
    }

    Ok(total)
}

/// Copy a directory recursively
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }

    Ok(())
}

/// Format bytes as human-readable string
pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} bytes", bytes)
    }
}

/// Format Unix timestamp as human-readable datetime
pub fn format_timestamp(timestamp: u64) -> String {
    // Simple formatting without chrono dependency
    let secs_since_epoch = timestamp;
    let days = secs_since_epoch / 86400;
    let remaining = secs_since_epoch % 86400;
    let hours = remaining / 3600;
    let minutes = (remaining % 3600) / 60;
    let seconds = remaining % 60;

    // Calculate year/month/day (simplified, doesn't account for leap years perfectly)
    let mut year = 1970;
    let mut remaining_days = days;

    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }

    let (month, day) = days_to_month_day(remaining_days as u32, is_leap_year(year));

    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        year, month, day, hours, minutes, seconds
    )
}

fn is_leap_year(year: u64) -> bool {
    year.is_multiple_of(4) && !year.is_multiple_of(100) || year.is_multiple_of(400)
}

fn days_to_month_day(days: u32, leap: bool) -> (u32, u32) {
    let days_in_months: [u32; 12] = if leap {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut remaining = days;
    for (i, &days_in_month) in days_in_months.iter().enumerate() {
        if remaining < days_in_month {
            return ((i + 1) as u32, remaining + 1);
        }
        remaining -= days_in_month;
    }

    (12, 31) // Fallback
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_db(path: &Path) -> DB {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        let db = DB::open(&opts, path).unwrap();

        // Add some test data
        db.put(b"key1", b"value1").unwrap();
        db.put(b"key2", b"value2").unwrap();
        db.put(b"key3", b"value3").unwrap();
        db.flush().unwrap();

        db
    }

    #[test]
    fn test_create_and_list_backup() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_db");
        let backup_dir = temp_dir.path().join("backups");

        // Create test database
        let _db = create_test_db(&db_path);
        drop(_db); // Close DB

        // Create backup manager
        let manager = BackupManager::new(&db_path, &backup_dir).unwrap();

        // Create backup (no flush needed since test DB already flushed)
        let backup_info = manager
            .create_backup(Some(BackupOptions {
                label: Some("test backup".to_string()),
                flush_before_backup: false,
            }))
            .unwrap();

        assert!(backup_info.id.starts_with("backup_"));
        assert_eq!(backup_info.label, Some("test backup".to_string()));
        assert!(backup_info.size_bytes > 0);

        // List backups
        let backups = manager.list_backups().unwrap();
        assert_eq!(backups.len(), 1);
        assert_eq!(backups[0].id, backup_info.id);
    }

    #[test]
    fn test_restore_backup() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_db");
        let backup_dir = temp_dir.path().join("backups");
        let restore_path = temp_dir.path().join("restored_db");

        // Create test database
        let _db = create_test_db(&db_path);
        drop(_db);

        // Create backup (no flush needed since test DB already flushed)
        let manager = BackupManager::new(&db_path, &backup_dir).unwrap();
        let backup_info = manager
            .create_backup(Some(BackupOptions {
                label: None,
                flush_before_backup: false,
            }))
            .unwrap();

        // Restore backup
        manager
            .restore_backup(&backup_info.id, &restore_path, false)
            .unwrap();

        // Verify restored data
        let mut opts = Options::default();
        opts.create_if_missing(false);
        let restored_db = DB::open(&opts, &restore_path).unwrap();

        assert_eq!(restored_db.get(b"key1").unwrap(), Some(b"value1".to_vec()));
        assert_eq!(restored_db.get(b"key2").unwrap(), Some(b"value2".to_vec()));
        assert_eq!(restored_db.get(b"key3").unwrap(), Some(b"value3".to_vec()));
    }

    #[test]
    fn test_delete_backup() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_db");
        let backup_dir = temp_dir.path().join("backups");

        let _db = create_test_db(&db_path);
        drop(_db);

        let manager = BackupManager::new(&db_path, &backup_dir).unwrap();
        let backup_info = manager
            .create_backup(Some(BackupOptions {
                label: None,
                flush_before_backup: false,
            }))
            .unwrap();

        assert_eq!(manager.list_backups().unwrap().len(), 1);

        manager.delete_backup(&backup_info.id).unwrap();

        assert_eq!(manager.list_backups().unwrap().len(), 0);
    }

    #[test]
    fn test_cleanup_old_backups() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_db");
        let backup_dir = temp_dir.path().join("backups");

        let _db = create_test_db(&db_path);
        drop(_db);

        let manager = BackupManager::new(&db_path, &backup_dir).unwrap();
        let opts = BackupOptions {
            label: None,
            flush_before_backup: false,
        };

        // Create multiple backups
        for _ in 0..5 {
            manager.create_backup(Some(opts.clone())).unwrap();
            std::thread::sleep(std::time::Duration::from_secs(1));
        }

        assert_eq!(manager.list_backups().unwrap().len(), 5);

        // Keep only 2 most recent
        let deleted = manager.cleanup_old_backups(2).unwrap();
        assert_eq!(deleted.len(), 3);
        assert_eq!(manager.list_backups().unwrap().len(), 2);
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(500), "500 bytes");
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(1536), "1.50 KB");
        assert_eq!(format_bytes(1048576), "1.00 MB");
        assert_eq!(format_bytes(1073741824), "1.00 GB");
    }

    #[test]
    fn test_backup_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_db");
        let backup_dir = temp_dir.path().join("backups");

        let _db = create_test_db(&db_path);
        drop(_db);

        let manager = BackupManager::new(&db_path, &backup_dir).unwrap();

        let result = manager.get_backup("nonexistent");
        assert!(matches!(result, Err(BackupError::BackupNotFound(_))));
    }

    // ----- format_bytes edges -------------------------------------------

    #[test]
    fn test_format_bytes_zero() {
        assert_eq!(format_bytes(0), "0 bytes");
    }

    #[test]
    fn test_format_bytes_unit_boundaries() {
        // Just under 1 KB stays in bytes
        assert_eq!(format_bytes(1023), "1023 bytes");
        // Exact unit boundaries
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(1024 * 1024), "1.00 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.00 GB");
        // Large value uses GB unit
        assert_eq!(format_bytes(2 * 1024 * 1024 * 1024), "2.00 GB");
    }

    // ----- format_timestamp ---------------------------------------------

    #[test]
    fn test_format_timestamp_epoch() {
        // Unix epoch: 1970-01-01 00:00:00 UTC
        assert_eq!(format_timestamp(0), "1970-01-01 00:00:00 UTC");
    }

    #[test]
    fn test_format_timestamp_known_value() {
        // 2024-01-01 00:00:00 UTC == 1704067200
        assert_eq!(
            format_timestamp(1_704_067_200),
            "2024-01-01 00:00:00 UTC",
            "leap-year handling for 2024 must round-trip"
        );
    }

    #[test]
    fn test_format_timestamp_non_leap_year_feb_28() {
        // 2023-02-28 12:34:56 UTC == 1677587696
        assert_eq!(format_timestamp(1_677_587_696), "2023-02-28 12:34:56 UTC");
    }

    #[test]
    fn test_format_timestamp_leap_day() {
        // 2024-02-29 00:00:00 UTC == 1709164800 (Feb 29 only exists in leap years)
        assert_eq!(
            format_timestamp(1_709_164_800),
            "2024-02-29 00:00:00 UTC",
            "leap day must be representable"
        );
    }

    // ----- list_backups edges -------------------------------------------

    #[test]
    fn test_list_backups_empty_dir_returns_empty() {
        let temp_dir = TempDir::new().unwrap();
        let backup_dir = temp_dir.path().join("backups_empty");
        let manager = BackupManager::new(temp_dir.path().join("db"), &backup_dir).unwrap();
        // BackupManager::new creates the directory; it must still report no backups.
        assert!(manager.list_backups().unwrap().is_empty());
    }

    #[test]
    fn test_list_backups_ignores_unrelated_directories() {
        let temp_dir = TempDir::new().unwrap();
        let backup_dir = temp_dir.path().join("backups");
        fs::create_dir_all(&backup_dir).unwrap();

        // Directories that don't start with `backup_` must be skipped silently.
        fs::create_dir_all(backup_dir.join("scratch")).unwrap();
        fs::create_dir_all(backup_dir.join("logs")).unwrap();

        let manager = BackupManager::new(temp_dir.path().join("db"), &backup_dir).unwrap();
        assert!(manager.list_backups().unwrap().is_empty());
    }

    #[test]
    fn test_list_backups_sorted_newest_first() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_db");
        let backup_dir = temp_dir.path().join("backups");

        let _db = create_test_db(&db_path);
        drop(_db);

        let manager = BackupManager::new(&db_path, &backup_dir).unwrap();
        let opts = BackupOptions {
            label: None,
            flush_before_backup: false,
        };

        let mut ids = Vec::new();
        for _ in 0..3 {
            ids.push(manager.create_backup(Some(opts.clone())).unwrap().id);
            std::thread::sleep(std::time::Duration::from_secs(1));
        }

        let listed = manager.list_backups().unwrap();
        assert_eq!(listed.len(), 3);
        // list_backups is sorted by created_at DESC
        for window in listed.windows(2) {
            assert!(window[0].created_at >= window[1].created_at);
        }
    }

    // ----- get_backup / delete_backup error paths -----------------------

    #[test]
    fn test_get_backup_unknown_id_returns_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("db");
        let backup_dir = temp_dir.path().join("backups");
        let _db = create_test_db(&db_path);
        drop(_db);

        let manager = BackupManager::new(&db_path, &backup_dir).unwrap();
        match manager.get_backup("backup_does_not_exist") {
            Err(BackupError::BackupNotFound(id)) => {
                assert_eq!(id, "backup_does_not_exist");
            }
            other => panic!("expected BackupNotFound, got {:?}", other),
        }
    }

    #[test]
    fn test_delete_backup_unknown_id_returns_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("db");
        let backup_dir = temp_dir.path().join("backups");
        let _db = create_test_db(&db_path);
        drop(_db);

        let manager = BackupManager::new(&db_path, &backup_dir).unwrap();
        assert!(matches!(
            manager.delete_backup("nope"),
            Err(BackupError::BackupNotFound(_))
        ));
    }

    // ----- restore_backup edges -----------------------------------------

    #[test]
    fn test_restore_backup_unknown_id_returns_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("db");
        let backup_dir = temp_dir.path().join("backups");
        let _db = create_test_db(&db_path);
        drop(_db);

        let manager = BackupManager::new(&db_path, &backup_dir).unwrap();
        let result = manager.restore_backup("missing", temp_dir.path().join("restored"), false);
        assert!(matches!(result, Err(BackupError::BackupNotFound(_))));
    }

    #[test]
    fn test_restore_backup_existing_target_without_overwrite() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("db");
        let backup_dir = temp_dir.path().join("backups");
        let restore_path = temp_dir.path().join("restored");

        let _db = create_test_db(&db_path);
        drop(_db);

        let manager = BackupManager::new(&db_path, &backup_dir).unwrap();
        let info = manager
            .create_backup(Some(BackupOptions {
                label: None,
                flush_before_backup: false,
            }))
            .unwrap();

        // Pre-create the restore target — restore must refuse without overwrite=true
        fs::create_dir_all(&restore_path).unwrap();
        fs::write(restore_path.join("sentinel"), b"in_use").unwrap();

        let err = manager
            .restore_backup(&info.id, &restore_path, false)
            .unwrap_err();
        assert!(matches!(err, BackupError::RestoreTargetExists(_)));
        // Sentinel must still be there (no destruction without permission)
        assert!(restore_path.join("sentinel").exists());
    }

    #[test]
    fn test_restore_backup_overwrite_replaces_target() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("db");
        let backup_dir = temp_dir.path().join("backups");
        let restore_path = temp_dir.path().join("restored");

        let _db = create_test_db(&db_path);
        drop(_db);

        let manager = BackupManager::new(&db_path, &backup_dir).unwrap();
        let info = manager
            .create_backup(Some(BackupOptions {
                label: None,
                flush_before_backup: false,
            }))
            .unwrap();

        // Pre-existing target with junk content
        fs::create_dir_all(&restore_path).unwrap();
        fs::write(restore_path.join("junk.txt"), b"stale").unwrap();

        manager
            .restore_backup(&info.id, &restore_path, true)
            .unwrap();

        // After overwrite, junk.txt must be gone and the restored DB must work
        assert!(
            !restore_path.join("junk.txt").exists(),
            "overwrite must wipe stale contents first"
        );

        let mut opts = Options::default();
        opts.create_if_missing(false);
        let restored = DB::open(&opts, &restore_path).unwrap();
        assert_eq!(restored.get(b"key1").unwrap(), Some(b"value1".to_vec()));
    }

    #[test]
    fn test_restored_db_strips_metadata_file() {
        // backup_metadata.json is internal — the restored directory must not
        // ship it (RocksDB would treat it as a stray file).
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("db");
        let backup_dir = temp_dir.path().join("backups");
        let restore_path = temp_dir.path().join("restored");

        let _db = create_test_db(&db_path);
        drop(_db);

        let manager = BackupManager::new(&db_path, &backup_dir).unwrap();
        let info = manager
            .create_backup(Some(BackupOptions {
                label: None,
                flush_before_backup: false,
            }))
            .unwrap();

        // The original backup directory keeps its metadata
        assert!(backup_dir
            .join(&info.id)
            .join("backup_metadata.json")
            .exists());

        manager
            .restore_backup(&info.id, &restore_path, false)
            .unwrap();

        // The restored copy must not retain it
        assert!(!restore_path.join("backup_metadata.json").exists());
    }

    // ----- cleanup_old_backups edges ------------------------------------

    #[test]
    fn test_cleanup_old_backups_keep_more_than_total_is_noop() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("db");
        let backup_dir = temp_dir.path().join("backups");
        let _db = create_test_db(&db_path);
        drop(_db);

        let manager = BackupManager::new(&db_path, &backup_dir).unwrap();
        let opts = BackupOptions {
            label: None,
            flush_before_backup: false,
        };
        manager.create_backup(Some(opts.clone())).unwrap();
        // backup IDs include a Unix-second timestamp; sleep to avoid collision
        std::thread::sleep(std::time::Duration::from_secs(1));
        manager.create_backup(Some(opts)).unwrap();

        let deleted = manager.cleanup_old_backups(10).unwrap();
        assert!(deleted.is_empty(), "keep_count > total must delete nothing");
        assert_eq!(manager.list_backups().unwrap().len(), 2);
    }

    #[test]
    fn test_cleanup_old_backups_keep_zero_deletes_all() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("db");
        let backup_dir = temp_dir.path().join("backups");
        let _db = create_test_db(&db_path);
        drop(_db);

        let manager = BackupManager::new(&db_path, &backup_dir).unwrap();
        let opts = BackupOptions {
            label: None,
            flush_before_backup: false,
        };
        manager.create_backup(Some(opts.clone())).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
        manager.create_backup(Some(opts)).unwrap();

        let deleted = manager.cleanup_old_backups(0).unwrap();
        assert_eq!(deleted.len(), 2);
        assert!(manager.list_backups().unwrap().is_empty());
    }

    // ----- BackupManager::new -------------------------------------------

    #[test]
    fn test_backup_manager_new_creates_missing_backup_dir() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("db");
        let backup_dir = temp_dir.path().join("a/b/c/backups");
        assert!(!backup_dir.exists());

        let _ = BackupManager::new(&db_path, &backup_dir).unwrap();
        assert!(backup_dir.exists());
    }

    // ----- BackupOptions defaults ---------------------------------------

    #[test]
    fn test_backup_options_default() {
        let opts = BackupOptions::default();
        assert!(opts.flush_before_backup);
        assert!(opts.label.is_none());
    }

    // ----- create_backup with default options -----------------------------

    #[test]
    fn test_create_backup_with_default_options_flushes() {
        // Default options request a flush-before-backup; this exercises the
        // read-write open path (different code branch from `flush=false`).
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("db");
        let backup_dir = temp_dir.path().join("backups");
        let _db = create_test_db(&db_path);
        drop(_db);

        let manager = BackupManager::new(&db_path, &backup_dir).unwrap();
        let info = manager.create_backup(None).unwrap(); // None -> defaults
        assert!(info.size_bytes > 0);
        assert!(backup_dir
            .join(&info.id)
            .join("backup_metadata.json")
            .exists());
    }
}

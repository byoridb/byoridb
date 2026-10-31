// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Backup and restore functionality for the redb-backed KVStore.
//!
//! redb is a single-file, pure-Rust store with built-in ACID durability
//! (every commit fsyncs). A consistent point-in-time backup is taken by
//! opening the stopped source under a read transaction (an MVCC snapshot) and
//! draining every key/value into a fresh redb file. Restore copies that
//! static file back into a target directory.
//!
//! The data directory contract matches [`crate::RedbKVStore`]: a *directory*
//! whose redb file is `data.redb`.
//!
//! # Usage
//!
//! ```ignore
//! use byoridb_kvstore::backup::{BackupManager, BackupOptions};
//!
//! let manager = BackupManager::new("/path/to/db", "/path/to/backups")?;
//! let backup_info = manager.create_backup(None)?;
//! let backups = manager.list_backups()?;
//! manager.restore_backup(&backup_info.id, "/path/to/restore", false)?;
//! ```

use redb::{Database, ReadableDatabase, TableDefinition, TableHandle};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tracing::{info, warn};

/// Single KV table — must match [`crate::store`]'s definition.
const KV_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("kv");
/// Bitemporal history table (T-track). Must be backed up alongside `kv`, else a
/// restored database loses all `AS OF` history while the current view survives.
const HISTORY_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("history");

/// redb data file name inside a data directory or backup directory.
const DATA_FILE: &str = "data.redb";

/// Backup-related errors
#[derive(Debug, Error)]
pub enum BackupError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("redb database error: {0}")]
    RedbDatabase(#[from] redb::DatabaseError),

    #[error("redb transaction error: {0}")]
    RedbTransaction(#[from] redb::TransactionError),

    #[error("redb table error: {0}")]
    RedbTable(#[from] redb::TableError),

    #[error("redb storage error: {0}")]
    RedbStorage(#[from] redb::StorageError),

    #[error("redb commit error: {0}")]
    RedbCommit(#[from] redb::CommitError),

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
    /// Unique backup identifier (timestamp plus exclusive random directory suffix)
    pub id: String,
    /// Timestamp when backup was created (Unix epoch seconds)
    pub created_at: u64,
    /// Exact size of data.redb in bytes (metadata is excluded)
    pub size_bytes: u64,
    /// Optional description/label
    pub label: Option<String>,
    /// Source database path
    pub source_path: String,
    /// Absent only on legacy archives created before integrity manifests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// Options for creating backups
#[derive(Debug, Clone)]
pub struct BackupOptions {
    /// Optional label for the backup
    pub label: Option<String>,
    /// Retained for API compatibility. redb fsyncs on every commit, so there
    /// is nothing extra to flush before a backup; this field is a no-op.
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

/// Manages backups for a redb database directory.
pub struct BackupManager {
    /// Path to the source database **directory** (contains `data.redb`).
    db_path: PathBuf,
    /// Path to the backup directory
    backup_dir: PathBuf,
}

impl BackupManager {
    /// Create a manager. Backup operations use a canonical root and reject
    /// explicit parent traversal and symlink leaves.
    pub fn new<P: AsRef<Path>, Q: AsRef<Path>>(db_path: P, backup_dir: Q) -> Result<Self> {
        reject_parent_components(db_path.as_ref())?;
        reject_parent_components(backup_dir.as_ref())?;
        reject_symlink(backup_dir.as_ref())?;
        if !backup_dir.as_ref().exists() {
            fs::create_dir_all(backup_dir.as_ref())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(backup_dir.as_ref(), fs::Permissions::from_mode(0o700))?;
            }
        }
        let backup_dir = fs::canonicalize(backup_dir)?;
        if !backup_dir.is_dir() {
            return Err(invalid("Backup root must be a directory"));
        }
        Ok(Self {
            db_path: db_path.as_ref().to_path_buf(),
            backup_dir,
        })
    }

    /// Snapshot a stopped database into an exclusively created archive.
    /// redb's exclusive file lock rejects a concurrently running server.
    pub fn create_backup(&self, options: Option<BackupOptions>) -> Result<BackupInfo> {
        let options = options.unwrap_or_default();
        reject_symlink(&self.db_path)?;
        require_regular_file(&self.db_path.join(DATA_FILE))?;
        let created = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| invalid(format!("Invalid system clock: {e}")))?;
        let timestamp = created.as_secs();
        // tempfile uses exclusive directory creation and retries collisions;
        // timestamps alone reused and overwrote archives within one second.
        let archive = tempfile::Builder::new()
            .prefix(&format!(
                "backup_{timestamp}_{:09}_",
                created.subsec_nanos()
            ))
            .rand_bytes(16)
            .tempdir_in(&self.backup_dir)?;
        let backup_id = archive
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let data_path = archive.path().join(DATA_FILE);
        snapshot_copy(&self.db_path.join(DATA_FILE), &data_path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&data_path, fs::Permissions::from_mode(0o600))?;
        }
        let backup_info = BackupInfo {
            id: backup_id,
            created_at: timestamp,
            size_bytes: fs::metadata(&data_path)?.len(),
            label: options.label,
            source_path: self.db_path.to_string_lossy().into_owned(),
            format_version: Some(1),
            data_file: Some(DATA_FILE.to_string()),
            sha256: Some(hash_file(&data_path)?),
        };
        verify_data(&data_path, &backup_info)?;
        self.save_backup_metadata(&backup_info)?;
        sync_dir(archive.path())?;
        sync_dir(&self.backup_dir)?;
        let _ = archive.keep();
        info!(
            "Backup '{}' created and verified ({} bytes)",
            backup_info.id, backup_info.size_bytes
        );
        Ok(backup_info)
    }

    /// List manifests. Malformed archive directories fail explicitly rather
    /// than disappearing from an operator's verify-all or retention run.
    pub fn list_backups(&self) -> Result<Vec<BackupInfo>> {
        let mut backups = Vec::new();
        for entry in fs::read_dir(&self.backup_dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("backup_") {
                backups.push(self.get_backup(&name)?);
            }
        }
        backups.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        Ok(backups)
    }

    /// Read and validate a manifest. This does not verify database bytes;
    /// use `verify_backup` before treating an archive as recoverable.
    pub fn get_backup(&self, backup_id: &str) -> Result<BackupInfo> {
        let backup_path = self.backup_path(backup_id)?;
        let info = self.load_backup_metadata(&backup_path)?;
        validate_manifest(&info, backup_id)?;
        Ok(info)
    }

    /// Check the manifest, exact file size, SHA-256 (new archives), redb page
    /// checksums and both table traversals. Legacy manifests lack a digest;
    /// they receive size and redb checks and are identified by version=None.
    /// Integrity checks run on a temporary copy and never repair the archive.
    pub fn verify_backup(&self, backup_id: &str) -> Result<BackupInfo> {
        let info = self.get_backup(backup_id)?;
        verify_data(&self.backup_path(backup_id)?.join(DATA_FILE), &info)?;
        Ok(info)
    }

    /// Validate and stage before changing an existing target. On promotion
    /// failure, the original directory is renamed back. The target must be
    /// stopped, outside the archive root, and explicitly opted into overwrite.
    pub fn restore_backup<P: AsRef<Path>>(
        &self,
        backup_id: &str,
        target_path: P,
        overwrite: bool,
    ) -> Result<()> {
        self.restore_with(
            backup_id,
            target_path.as_ref(),
            overwrite,
            |src, dst| {
                fs::copy(src, dst)?;
                Ok(())
            },
            |src, dst| fs::rename(src, dst),
        )
    }

    fn restore_with<C, P>(
        &self,
        backup_id: &str,
        target: &Path,
        overwrite: bool,
        copy: C,
        promote: P,
    ) -> Result<()>
    where
        C: FnOnce(&Path, &Path) -> std::io::Result<()>,
        P: FnOnce(&Path, &Path) -> std::io::Result<()>,
    {
        // Invalid archives must not create, remove or rename the target.
        let info = self.verify_backup(backup_id)?;
        reject_parent_components(target)?;
        reject_symlink(target)?;
        let target = resolved_target(target)?;
        if target.starts_with(&self.backup_dir) || self.backup_dir.starts_with(&target) {
            return Err(invalid("Restore target and backup root must not overlap"));
        }
        let existed = target.try_exists()?;
        if existed && !overwrite {
            return Err(BackupError::RestoreTargetExists(
                target.display().to_string(),
            ));
        }
        if existed && !target.is_dir() {
            return Err(invalid("Restore target must be a directory"));
        }
        // Hold redb's OS-level lock until promotion finishes, without opening
        // or repairing the old database. A corrupt stopped DB can be replaced.
        let old_data = target.join(DATA_FILE);
        let _target_lock = if old_data.try_exists()? {
            require_regular_file(&old_data)?;
            let file = File::options().read(true).write(true).open(&old_data)?;
            file.try_lock()
                .map_err(|e| invalid(format!("Target database must be stopped: {e}")))?;
            Some(file)
        } else {
            None
        };
        let parent = target
            .parent()
            .ok_or_else(|| invalid("Target has no parent"))?;
        fs::create_dir_all(parent)?;
        let staging = tempfile::Builder::new()
            .prefix(".byoridb-restore-")
            .tempdir_in(parent)?;
        let ready = staging.path().join("ready");
        let previous = staging.path().join("previous");
        fs::create_dir(&ready)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&ready, fs::Permissions::from_mode(0o700))?;
        }
        let data_path = ready.join(DATA_FILE);
        copy(&self.backup_path(backup_id)?.join(DATA_FILE), &data_path)?;
        verify_data(&data_path, &info)?;
        File::open(&data_path)?.sync_all()?;
        sync_dir(&ready)?;
        // Recheck before mutation; no-overwrite must also reject a target that
        // appeared while the archive was being copied.
        if target.try_exists()? != existed {
            return Err(invalid("Restore target changed while staging"));
        }
        reject_symlink(&target)?;
        if existed {
            fs::rename(&target, &previous)?;
            // Persist the old target's recovery location before publishing the
            // replacement. A process interruption can be recovered manually
            // from `previous` while the two directory renames are in flight.
            if let Err(error) = sync_dir(staging.path()).and_then(|_| sync_dir(parent)) {
                if let Err(rollback) = fs::rename(&previous, &target) {
                    let preserved = staging.keep();
                    return Err(invalid(format!(
                        "Restore staging sync failed: {error}; rollback failed: {rollback}; original target preserved at {}",
                        preserved.join("previous").display()
                    )));
                }
                return Err(error);
            }
        }
        if let Err(error) = promote(&ready, &target) {
            if existed {
                if let Err(rollback) = fs::rename(&previous, &target) {
                    let preserved = staging.keep();
                    return Err(invalid(format!("Promotion failed: {error}; rollback failed: {rollback}; original target preserved at {}", preserved.join("previous").display())));
                }
            }
            return Err(error.into());
        }
        if let Err(error) = sync_dir(parent).and_then(|_| sync_dir(staging.path())) {
            // Keep both recoverable copies if durability cannot be confirmed.
            if fs::rename(&target, &ready).is_err()
                || (existed && fs::rename(&previous, &target).is_err())
            {
                let preserved = staging.keep();
                return Err(invalid(format!(
                    "Restore directory sync failed: {error}; recovery files preserved at {}",
                    preserved.display()
                )));
            }
            return Err(error);
        }
        // This is the commit point. Failure to remove the old target is not a
        // restore failure; any residual staging files can be removed manually.
        drop(_target_lock);
        if let Err(error) = staging.close() {
            warn!(
                "Restored target, but could not remove staging directory: {}",
                error
            );
        }
        info!("Backup '{}' restored to {:?}", backup_id, target);
        Ok(())
    }

    pub fn delete_backup(&self, backup_id: &str) -> Result<()> {
        fs::remove_dir_all(self.backup_path(backup_id)?)?;
        Ok(())
    }

    pub fn cleanup_old_backups(&self, keep_count: usize) -> Result<Vec<String>> {
        let mut deleted = Vec::new();
        for backup in self.list_backups()?.into_iter().skip(keep_count) {
            self.delete_backup(&backup.id)?;
            deleted.push(backup.id);
        }
        Ok(deleted)
    }

    fn backup_path(&self, backup_id: &str) -> Result<PathBuf> {
        if backup_id.is_empty()
            || !backup_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(invalid("Backup ID must be one directory name"));
        }
        let path = self.backup_dir.join(backup_id);
        reject_symlink(&path)?;
        if !path.try_exists()? {
            return Err(BackupError::BackupNotFound(backup_id.into()));
        }
        if !path.is_dir() {
            return Err(invalid("Backup must be a directory"));
        }
        Ok(path)
    }

    fn save_backup_metadata(&self, info: &BackupInfo) -> Result<()> {
        validate_manifest(info, &info.id)?;
        let bytes = serde_json::to_vec_pretty(info)
            .map_err(|e| BackupError::Serialization(e.to_string()))?;
        if bytes.len() > 1024 * 1024 {
            return Err(invalid("Backup manifest exceeds 1 MiB"));
        }
        let file = File::create_new(self.backup_path(&info.id)?.join("backup_metadata.json"))?;
        let mut writer = BufWriter::new(file);
        writer.write_all(&bytes)?;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        Ok(())
    }

    fn load_backup_metadata(&self, backup_path: &Path) -> Result<BackupInfo> {
        let path = backup_path.join("backup_metadata.json");
        require_regular_file(&path)?;
        if fs::metadata(&path)?.len() > 1024 * 1024 {
            return Err(invalid("Backup manifest exceeds 1 MiB"));
        }
        serde_json::from_reader(BufReader::new(File::open(path)?))
            .map_err(|e| BackupError::Serialization(e.to_string()))
    }
}

fn invalid(message: impl Into<String>) -> BackupError {
    BackupError::InvalidBackup(message.into())
}

fn reject_parent_components(path: &Path) -> Result<()> {
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(invalid("Parent traversal is not allowed in backup paths"));
    }
    Ok(())
}

fn reject_symlink(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => Err(invalid(format!(
            "Symlinks are not allowed: {}",
            path.display()
        ))),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

fn require_regular_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(invalid(format!(
            "Expected regular file: {}",
            path.display()
        )));
    }
    Ok(())
}

// Resolve existing ancestors before overlap checks, including OS aliases such
// as /tmp -> /private/tmp. Only missing suffixes are appended afterwards.
fn resolved_target(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut ancestor = absolute.as_path();
    let mut missing = Vec::new();
    while !ancestor.try_exists()? {
        missing.push(
            ancestor
                .file_name()
                .ok_or_else(|| invalid("Invalid restore path"))?
                .to_os_string(),
        );
        ancestor = ancestor
            .parent()
            .ok_or_else(|| invalid("Invalid restore path"))?;
    }
    let mut resolved = fs::canonicalize(ancestor)?;
    for part in missing.into_iter().rev() {
        resolved.push(part);
    }
    if resolved.parent().is_none() {
        return Err(invalid("Cannot restore over filesystem root"));
    }
    Ok(resolved)
}

fn validate_manifest(info: &BackupInfo, id: &str) -> Result<()> {
    if info.id != id || info.size_bytes == 0 || info.created_at > 253_402_300_799 {
        return Err(invalid("Manifest ID, timestamp or data size is invalid"));
    }
    match (
        info.format_version,
        info.data_file.as_deref(),
        info.sha256.as_deref(),
    ) {
        (None, None, None) => Ok(()),
        (Some(1), Some(DATA_FILE), Some(hash))
            if hash.len() == 64
                && hash
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) =>
        {
            Ok(())
        }
        _ => Err(invalid("Unsupported or incomplete backup manifest format")),
    }
}

fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn verify_data(path: &Path, info: &BackupInfo) -> Result<()> {
    require_regular_file(path)?;
    if fs::metadata(path)?.len() != info.size_bytes {
        return Err(invalid("Backup data size does not match manifest"));
    }
    let scratch = tempfile::Builder::new()
        .prefix("byoridb-verify-")
        .tempdir()?;
    let copy = scratch.path().join(DATA_FILE);
    fs::copy(path, &copy)?;
    if fs::metadata(&copy)?.len() != info.size_bytes {
        return Err(invalid("Backup changed while being verified"));
    }
    if let Some(expected) = &info.sha256 {
        if &hash_file(&copy)? != expected {
            return Err(invalid("Backup SHA-256 does not match manifest"));
        }
    }
    // redb 4.1 can panic in Database::drop after detecting corrupt pages.
    // Keep the entire temporary database lifetime inside the unwind boundary;
    // a rejected archive is an error, never a process-wide verification crash.
    std::panic::catch_unwind(|| verify_redb_copy(&copy, info))
        .map_err(|_| invalid("redb rejected corrupt backup during integrity checking"))?
}

fn verify_redb_copy(copy: &Path, info: &BackupInfo) -> Result<()> {
    // Redb may repair during open as well as check_integrity. Abort automatic
    // repair on open, and reject check_integrity's repaired (false) result.
    let mut builder = redb::Builder::new();
    builder.set_repair_callback(|session| session.abort());
    let mut db = builder.open(copy)?;
    if !db.check_integrity()? {
        return Err(invalid("Backup requires redb repair"));
    }
    let read = db.begin_read()?;
    let names = read
        .list_tables()?
        .map(|t| t.name().to_string())
        .collect::<Vec<_>>();
    if !names.iter().any(|name| name == "kv")
        || names.iter().any(|name| name != "kv" && name != "history")
        || read.list_multimap_tables()?.next().is_some()
    {
        return Err(invalid("Backup database has missing or unsupported tables"));
    }
    for table in [KV_TABLE, HISTORY_TABLE] {
        match read.open_table(table) {
            Ok(table) => {
                for row in table.range::<&[u8]>(..)? {
                    let _ = row?;
                }
            }
            Err(redb::TableError::TableDoesNotExist(_)) if info.format_version.is_none() => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Open `src` redb under a read snapshot and drain every key/value into a
/// freshly created `dst` redb file. This yields a consistent, self-contained
/// copy. Opening a database held by a running server fails its exclusive lock.
fn snapshot_copy(src: &Path, dst: &Path) -> Result<()> {
    // Read-only open holds a shared lock (excluding redb writers) without
    // changing source bytes. Check its physical pages before copying records:
    // otherwise corrupted source values could become a clean-looking archive.
    let source = redb::ReadOnlyDatabase::open(src)?;
    let source_info = BackupInfo {
        id: "source".into(),
        created_at: 0,
        size_bytes: fs::metadata(src)?.len(),
        label: None,
        source_path: src.to_string_lossy().into_owned(),
        format_version: None,
        data_file: None,
        sha256: None,
    };
    verify_data(src, &source_info)?;
    let rtx = source.begin_read()?;
    if rtx
        .list_tables()?
        .any(|table| table.name() != "kv" && table.name() != "history")
        || rtx.list_multimap_tables()?.next().is_some()
    {
        return Err(invalid("Source database contains unsupported tables"));
    }

    let dest = Database::create(dst)?;
    let wtx = dest.begin_write()?;
    // Copy every persisted table: the current view (`kv`) AND the bitemporal
    // history (`history`). Copying only `kv` silently dropped all AS OF history.
    for table in [KV_TABLE, HISTORY_TABLE] {
        let mut dst_table = wtx.open_table(table)?;
        match rtx.open_table(table) {
            Ok(src_table) => {
                for entry in src_table.range::<&[u8]>(..)? {
                    let (k, v) = entry?;
                    dst_table.insert(k.value(), v.value())?;
                }
            }
            // A pristine source that never wrote the table is a valid empty DB.
            Err(redb::TableError::TableDoesNotExist(_)) => {}
            Err(e) => return Err(e.into()),
        }
    }
    wtx.commit()?;
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

    /// Create a redb data directory (`<dir>/data.redb`) with test rows.
    fn create_test_db(dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        let db = Database::create(dir.join(DATA_FILE)).unwrap();
        let wtx = db.begin_write().unwrap();
        {
            let mut t = wtx.open_table(KV_TABLE).unwrap();
            t.insert(b"key1".as_slice(), b"value1".as_slice()).unwrap();
            t.insert(b"key2".as_slice(), b"value2".as_slice()).unwrap();
            t.insert(b"key3".as_slice(), b"value3".as_slice()).unwrap();
        }
        wtx.commit().unwrap();
    }

    fn read_key(dir: &Path, key: &[u8]) -> Option<Vec<u8>> {
        let db = Database::open(dir.join(DATA_FILE)).unwrap();
        let rtx = db.begin_read().unwrap();
        let t = rtx.open_table(KV_TABLE).unwrap();
        t.get(key).unwrap().map(|g| g.value().to_vec())
    }

    /// Regression: a backup must preserve the bitemporal `history` table, not
    /// just the current-view `kv` table. Previously only `kv` was copied, so a
    /// restored DB kept the current view but lost all `AS OF` history.
    #[test]
    fn test_snapshot_copy_includes_history_table() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src.redb");
        let dst = temp.path().join("dst.redb");
        {
            let db = Database::create(&src).unwrap();
            let wtx = db.begin_write().unwrap();
            {
                let mut kv = wtx.open_table(KV_TABLE).unwrap();
                kv.insert(b"cur".as_slice(), b"v".as_slice()).unwrap();
            }
            {
                let mut h = wtx.open_table(HISTORY_TABLE).unwrap();
                h.insert(b"hist-key".as_slice(), b"hist-val".as_slice())
                    .unwrap();
            }
            wtx.commit().unwrap();
        }

        snapshot_copy(&src, &dst).unwrap();

        let db = Database::open(&dst).unwrap();
        let rtx = db.begin_read().unwrap();
        let h = rtx.open_table(HISTORY_TABLE).unwrap();
        assert_eq!(
            h.get(b"hist-key".as_slice())
                .unwrap()
                .map(|g| g.value().to_vec()),
            Some(b"hist-val".to_vec()),
            "HISTORY_TABLE must be preserved by snapshot_copy"
        );
    }

    #[test]
    fn test_create_and_list_backup() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_db");
        let backup_dir = temp_dir.path().join("backups");

        create_test_db(&db_path);

        let manager = BackupManager::new(&db_path, &backup_dir).unwrap();

        let backup_info = manager
            .create_backup(Some(BackupOptions {
                label: Some("test backup".to_string()),
                flush_before_backup: false,
            }))
            .unwrap();

        assert!(backup_info.id.starts_with("backup_"));
        assert_eq!(backup_info.label, Some("test backup".to_string()));
        assert!(backup_info.size_bytes > 0);

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

        create_test_db(&db_path);

        let manager = BackupManager::new(&db_path, &backup_dir).unwrap();
        let backup_info = manager
            .create_backup(Some(BackupOptions {
                label: None,
                flush_before_backup: false,
            }))
            .unwrap();

        manager
            .restore_backup(&backup_info.id, &restore_path, false)
            .unwrap();

        assert_eq!(read_key(&restore_path, b"key1"), Some(b"value1".to_vec()));
        assert_eq!(read_key(&restore_path, b"key2"), Some(b"value2".to_vec()));
        assert_eq!(read_key(&restore_path, b"key3"), Some(b"value3".to_vec()));
    }

    #[test]
    fn test_delete_backup() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_db");
        let backup_dir = temp_dir.path().join("backups");

        create_test_db(&db_path);

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

        create_test_db(&db_path);

        let manager = BackupManager::new(&db_path, &backup_dir).unwrap();
        let opts = BackupOptions {
            label: None,
            flush_before_backup: false,
        };

        for _ in 0..5 {
            manager.create_backup(Some(opts.clone())).unwrap();
            std::thread::sleep(std::time::Duration::from_secs(1));
        }

        assert_eq!(manager.list_backups().unwrap().len(), 5);

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

        create_test_db(&db_path);

        let manager = BackupManager::new(&db_path, &backup_dir).unwrap();

        let result = manager.get_backup("nonexistent");
        assert!(matches!(result, Err(BackupError::BackupNotFound(_))));
    }

    #[test]
    fn regression_rapid_backups_never_overwrite() {
        let temp = TempDir::new().unwrap();
        let db = temp.path().join("db");
        create_test_db(&db);
        let manager = BackupManager::new(&db, temp.path().join("archives")).unwrap();
        let first = manager.create_backup(None).unwrap();
        let first_bytes = fs::read(manager.backup_dir.join(&first.id).join(DATA_FILE)).unwrap();
        let second = manager.create_backup(None).unwrap();
        assert_ne!(first.id, second.id);
        assert_eq!(
            fs::read(manager.backup_dir.join(&first.id).join(DATA_FILE)).unwrap(),
            first_bytes
        );
        assert_eq!(manager.list_backups().unwrap().len(), 2);
        assert_eq!(manager.list_backups().unwrap()[0].id, second.id);
        assert_eq!(manager.cleanup_old_backups(1).unwrap(), vec![first.id]);
    }

    #[test]
    fn regression_archive_paths_cannot_escape_root() {
        let temp = TempDir::new().unwrap();
        let outside = temp.path().join("outside");
        create_test_db(&outside);
        let manager = BackupManager::new(&outside, temp.path().join("archives")).unwrap();
        assert!(manager.delete_backup("../outside").is_err());
        assert!(outside.join(DATA_FILE).exists());
    }

    #[test]
    fn regression_invalid_metadata_cannot_be_reconstructed() {
        let temp = TempDir::new().unwrap();
        let db = temp.path().join("db");
        create_test_db(&db);
        let manager = BackupManager::new(&db, temp.path().join("archives")).unwrap();
        let info = manager.create_backup(None).unwrap();
        fs::write(
            manager
                .backup_dir
                .join(&info.id)
                .join("backup_metadata.json"),
            b"garbage",
        )
        .unwrap();
        assert!(manager.get_backup(&info.id).is_err());
    }

    fn fixture() -> (TempDir, BackupManager, BackupInfo) {
        let temp = TempDir::new().unwrap();
        let db = temp.path().join("db");
        create_test_db(&db);
        let manager = BackupManager::new(&db, temp.path().join("archives")).unwrap();
        let info = manager.create_backup(None).unwrap();
        (temp, manager, info)
    }

    fn write_manifest(manager: &BackupManager, info: &BackupInfo) {
        fs::write(
            manager
                .backup_dir
                .join(&info.id)
                .join("backup_metadata.json"),
            serde_json::to_vec(info).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn verification_rejects_missing_truncated_and_corrupt_data_without_mutating_it() {
        for mode in ["missing", "truncate", "corrupt"] {
            let (_temp, manager, info) = fixture();
            let data = manager.backup_dir.join(&info.id).join(DATA_FILE);
            match mode {
                "missing" => fs::remove_file(&data).unwrap(),
                "truncate" => File::options()
                    .write(true)
                    .open(&data)
                    .unwrap()
                    .set_len(32)
                    .unwrap(),
                _ => {
                    let mut bytes = fs::read(&data).unwrap();
                    bytes[0] ^= 0xff;
                    fs::write(&data, bytes).unwrap();
                }
            }
            let before = fs::read(&data).ok();
            assert!(manager.verify_backup(&info.id).is_err(), "{mode}");
            assert_eq!(
                fs::read(&data).ok(),
                before,
                "verification must never repair original bytes"
            );
        }
    }

    #[test]
    fn legacy_manifest_is_verified_without_a_digest_but_corrupt_content_fails() {
        let (_temp, manager, mut info) = fixture();
        info.format_version = None;
        info.data_file = None;
        info.sha256 = None;
        write_manifest(&manager, &info);
        assert!(manager
            .verify_backup(&info.id)
            .unwrap()
            .format_version
            .is_none());
        let data = manager.backup_dir.join(&info.id).join(DATA_FILE);
        let mut bytes = fs::read(&data).unwrap();
        bytes[0] ^= 0xff;
        fs::write(&data, &bytes).unwrap();
        assert!(manager.verify_backup(&info.id).is_err());
        assert_eq!(fs::read(&data).unwrap(), bytes);
    }

    #[test]
    fn database_page_corruption_is_rejected_even_with_a_matching_digest() {
        let (_temp, manager, mut info) = fixture();
        let data = manager.backup_dir.join(&info.id).join(DATA_FILE);
        let mut bytes = fs::read(&data).unwrap();
        let index = bytes
            .windows(6)
            .position(|w| w == b"value1")
            .expect("fixture payload must be in a data page");
        bytes[index] ^= 1;
        fs::write(&data, &bytes).unwrap();
        info.sha256 = Some(hash_file(&data).unwrap());
        write_manifest(&manager, &info);
        assert!(
            manager.verify_backup(&info.id).is_err(),
            "SHA-256 agreement does not replace redb page checks"
        );
        assert_eq!(
            fs::read(data).unwrap(),
            bytes,
            "repair must never change archived bytes"
        );
    }

    #[test]
    fn corrupt_source_is_not_repaired_or_laundered_into_a_valid_backup() {
        let (_temp, manager, _info) = fixture();
        let source = manager.db_path.join(DATA_FILE);
        let mut bytes = fs::read(&source).unwrap();
        let index = bytes.windows(6).position(|w| w == b"value1").unwrap();
        bytes[index] ^= 1;
        fs::write(&source, &bytes).unwrap();
        assert!(manager.create_backup(None).is_err());
        assert_eq!(fs::read(source).unwrap(), bytes);
        assert_eq!(
            manager.list_backups().unwrap().len(),
            1,
            "failed backup must clean up its partial directory"
        );
    }

    #[test]
    fn legacy_kv_only_archives_restore_but_version_one_requires_history_table() {
        let temp = TempDir::new().unwrap();
        let db = temp.path().join("source");
        create_test_db(&db);
        let manager = BackupManager::new(&db, temp.path().join("archives")).unwrap();
        let archive = manager.backup_dir.join("backup_1");
        fs::create_dir(&archive).unwrap();
        fs::copy(db.join(DATA_FILE), archive.join(DATA_FILE)).unwrap();
        let mut info = BackupInfo {
            id: "backup_1".into(),
            created_at: 1,
            size_bytes: fs::metadata(archive.join(DATA_FILE)).unwrap().len(),
            label: None,
            source_path: db.to_string_lossy().into_owned(),
            format_version: Some(1),
            data_file: Some(DATA_FILE.into()),
            sha256: Some(hash_file(&archive.join(DATA_FILE)).unwrap()),
        };
        write_manifest(&manager, &info);
        assert!(manager.verify_backup(&info.id).is_err());
        info.format_version = None;
        info.data_file = None;
        info.sha256 = None;
        write_manifest(&manager, &info);
        manager.verify_backup(&info.id).unwrap();
        let target = temp.path().join("legacy_restored");
        manager.restore_backup(&info.id, &target, false).unwrap();
        assert_eq!(read_key(&target, b"key1"), Some(b"value1".to_vec()));
    }

    #[test]
    fn invalid_manifest_id_version_filename_digest_and_size_fail_closed() {
        let (_temp, manager, good) = fixture();
        for mode in ["id", "version", "filename", "digest", "size", "timestamp"] {
            let mut info = good.clone();
            match mode {
                "id" => info.id = "another_archive".into(),
                "version" => info.format_version = Some(999),
                "filename" => info.data_file = Some("../data.redb".into()),
                "digest" => info.sha256 = Some("0".repeat(64)),
                "timestamp" => info.created_at = u64::MAX,
                _ => info.size_bytes += 1,
            }
            fs::write(
                manager
                    .backup_dir
                    .join(&good.id)
                    .join("backup_metadata.json"),
                serde_json::to_vec(&info).unwrap(),
            )
            .unwrap();
            assert!(manager.verify_backup(&good.id).is_err(), "{mode}");
        }
    }

    #[test]
    fn list_and_verify_do_not_hide_malformed_archives() {
        let (_temp, manager, info) = fixture();
        fs::remove_file(
            manager
                .backup_dir
                .join(&info.id)
                .join("backup_metadata.json"),
        )
        .unwrap();
        assert!(manager.list_backups().is_err());
        assert!(manager.verify_backup(&info.id).is_err());
    }

    #[test]
    fn restore_validation_failure_preserves_existing_target() {
        let (temp, manager, info) = fixture();
        let target = temp.path().join("target");
        create_test_db(&target);
        fs::write(target.join("sentinel"), b"original").unwrap();
        let original = fs::read(target.join(DATA_FILE)).unwrap();
        fs::remove_file(manager.backup_dir.join(&info.id).join(DATA_FILE)).unwrap();
        assert!(manager.restore_backup(&info.id, &target, true).is_err());
        assert_eq!(fs::read(target.join(DATA_FILE)).unwrap(), original);
        assert_eq!(fs::read(target.join("sentinel")).unwrap(), b"original");
    }

    #[test]
    fn restore_partial_copy_and_staging_validation_failure_preserve_target() {
        for copy_errors in [true, false] {
            let (temp, manager, info) = fixture();
            let target = temp.path().join("target");
            create_test_db(&target);
            let original = fs::read(target.join(DATA_FILE)).unwrap();
            let result = manager.restore_with(
                &info.id,
                &target,
                true,
                |_, destination| {
                    fs::write(destination, b"partial copy")?;
                    if copy_errors {
                        Err(std::io::Error::other("injected disk full"))
                    } else {
                        Ok(())
                    }
                },
                |_, _| panic!("invalid staging must never be promoted"),
            );
            assert!(result.is_err());
            assert_eq!(fs::read(target.join(DATA_FILE)).unwrap(), original);
        }
    }

    #[test]
    fn restore_promotion_failure_rolls_back_original_target() {
        let (temp, manager, info) = fixture();
        let target = temp.path().join("target");
        create_test_db(&target);
        fs::write(target.join("sentinel"), b"original").unwrap();
        let original = fs::read(target.join(DATA_FILE)).unwrap();
        assert!(manager
            .restore_with(
                &info.id,
                &target,
                true,
                |src, dst| {
                    fs::copy(src, dst)?;
                    Ok(())
                },
                |_, _| Err(std::io::Error::other("injected rename failure"))
            )
            .is_err());
        assert_eq!(fs::read(target.join(DATA_FILE)).unwrap(), original);
        assert_eq!(fs::read(target.join("sentinel")).unwrap(), b"original");
    }

    #[test]
    fn restore_refuses_archive_root_and_ancestors_or_descendants() {
        let (temp, manager, info) = fixture();
        for target in [
            temp.path().to_path_buf(),
            manager.backup_dir.clone(),
            manager.backup_dir.join("new_target"),
            manager.backup_dir.join(&info.id),
        ] {
            assert!(manager.restore_backup(&info.id, target, true).is_err());
            assert!(manager.verify_backup(&info.id).is_ok());
        }
    }

    #[test]
    fn backup_and_restore_refuse_a_live_database_lock() {
        let (temp, manager, info) = fixture();
        let source = Database::open(manager.db_path.join(DATA_FILE)).unwrap();
        assert!(manager.create_backup(None).is_err());
        drop(source);
        let target = temp.path().join("target");
        create_test_db(&target);
        let live = Database::open(target.join(DATA_FILE)).unwrap();
        assert!(manager.restore_backup(&info.id, &target, true).is_err());
        drop(live);
        assert_eq!(read_key(&target, b"key1"), Some(b"value1".to_vec()));
    }

    #[cfg(unix)]
    #[test]
    fn archive_data_manifest_and_restore_symlinks_are_rejected() {
        use std::os::unix::fs::symlink;
        for mode in ["directory", "data", "metadata", "target"] {
            let (temp, manager, info) = fixture();
            let archive = manager.backup_dir.join(&info.id);
            match mode {
                "directory" => {
                    symlink(&archive, manager.backup_dir.join("backup_link")).unwrap();
                    assert!(manager.verify_backup("backup_link").is_err());
                    assert!(manager.delete_backup("backup_link").is_err());
                }
                "data" | "metadata" => {
                    let file = archive.join(if mode == "data" {
                        DATA_FILE
                    } else {
                        "backup_metadata.json"
                    });
                    let outside = temp.path().join("outside");
                    fs::rename(&file, &outside).unwrap();
                    symlink(&outside, &file).unwrap();
                    assert!(manager.verify_backup(&info.id).is_err());
                    assert!(outside.exists());
                }
                _ => {
                    let outside = temp.path().join("outside");
                    create_test_db(&outside);
                    let target = temp.path().join("target");
                    symlink(&outside, &target).unwrap();
                    assert!(manager.restore_backup(&info.id, &target, true).is_err());
                    assert!(outside.join(DATA_FILE).exists());
                }
            }
        }
    }

    // ----- format_bytes edges -------------------------------------------

    #[test]
    fn test_format_bytes_zero() {
        assert_eq!(format_bytes(0), "0 bytes");
    }

    #[test]
    fn test_format_bytes_unit_boundaries() {
        assert_eq!(format_bytes(1023), "1023 bytes");
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(1024 * 1024), "1.00 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.00 GB");
        assert_eq!(format_bytes(2 * 1024 * 1024 * 1024), "2.00 GB");
    }

    // ----- format_timestamp ---------------------------------------------

    #[test]
    fn test_format_timestamp_epoch() {
        assert_eq!(format_timestamp(0), "1970-01-01 00:00:00 UTC");
    }

    #[test]
    fn test_format_timestamp_known_value() {
        assert_eq!(
            format_timestamp(1_704_067_200),
            "2024-01-01 00:00:00 UTC",
            "leap-year handling for 2024 must round-trip"
        );
    }

    #[test]
    fn test_format_timestamp_non_leap_year_feb_28() {
        assert_eq!(format_timestamp(1_677_587_696), "2023-02-28 12:34:56 UTC");
    }

    #[test]
    fn test_format_timestamp_leap_day() {
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
        assert!(manager.list_backups().unwrap().is_empty());
    }

    #[test]
    fn test_list_backups_ignores_unrelated_directories() {
        let temp_dir = TempDir::new().unwrap();
        let backup_dir = temp_dir.path().join("backups");
        fs::create_dir_all(&backup_dir).unwrap();

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

        create_test_db(&db_path);

        let manager = BackupManager::new(&db_path, &backup_dir).unwrap();
        let opts = BackupOptions {
            label: None,
            flush_before_backup: false,
        };

        for _ in 0..3 {
            manager.create_backup(Some(opts.clone())).unwrap();
            std::thread::sleep(std::time::Duration::from_secs(1));
        }

        let listed = manager.list_backups().unwrap();
        assert_eq!(listed.len(), 3);
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
        create_test_db(&db_path);

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
        create_test_db(&db_path);

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
        create_test_db(&db_path);

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

        create_test_db(&db_path);

        let manager = BackupManager::new(&db_path, &backup_dir).unwrap();
        let info = manager
            .create_backup(Some(BackupOptions {
                label: None,
                flush_before_backup: false,
            }))
            .unwrap();

        fs::create_dir_all(&restore_path).unwrap();
        fs::write(restore_path.join("sentinel"), b"in_use").unwrap();

        let err = manager
            .restore_backup(&info.id, &restore_path, false)
            .unwrap_err();
        assert!(matches!(err, BackupError::RestoreTargetExists(_)));
        assert!(restore_path.join("sentinel").exists());
    }

    #[test]
    fn test_restore_backup_overwrite_replaces_target() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("db");
        let backup_dir = temp_dir.path().join("backups");
        let restore_path = temp_dir.path().join("restored");

        create_test_db(&db_path);

        let manager = BackupManager::new(&db_path, &backup_dir).unwrap();
        let info = manager
            .create_backup(Some(BackupOptions {
                label: None,
                flush_before_backup: false,
            }))
            .unwrap();

        fs::create_dir_all(&restore_path).unwrap();
        fs::write(restore_path.join("junk.txt"), b"stale").unwrap();

        manager
            .restore_backup(&info.id, &restore_path, true)
            .unwrap();

        assert!(
            !restore_path.join("junk.txt").exists(),
            "overwrite must wipe stale contents first"
        );
        assert_eq!(read_key(&restore_path, b"key1"), Some(b"value1".to_vec()));
    }

    #[test]
    fn test_restored_db_strips_metadata_file() {
        // Only data.redb is copied on restore — metadata stays in the backup dir.
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("db");
        let backup_dir = temp_dir.path().join("backups");
        let restore_path = temp_dir.path().join("restored");

        create_test_db(&db_path);

        let manager = BackupManager::new(&db_path, &backup_dir).unwrap();
        let info = manager
            .create_backup(Some(BackupOptions {
                label: None,
                flush_before_backup: false,
            }))
            .unwrap();

        assert!(backup_dir
            .join(&info.id)
            .join("backup_metadata.json")
            .exists());

        manager
            .restore_backup(&info.id, &restore_path, false)
            .unwrap();

        assert!(!restore_path.join("backup_metadata.json").exists());
    }

    // ----- cleanup_old_backups edges ------------------------------------

    #[test]
    fn test_cleanup_old_backups_keep_more_than_total_is_noop() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("db");
        let backup_dir = temp_dir.path().join("backups");
        create_test_db(&db_path);

        let manager = BackupManager::new(&db_path, &backup_dir).unwrap();
        let opts = BackupOptions {
            label: None,
            flush_before_backup: false,
        };
        manager.create_backup(Some(opts.clone())).unwrap();
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
        create_test_db(&db_path);

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

    #[test]
    fn test_create_backup_with_default_options() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("db");
        let backup_dir = temp_dir.path().join("backups");
        create_test_db(&db_path);

        let manager = BackupManager::new(&db_path, &backup_dir).unwrap();
        let info = manager.create_backup(None).unwrap(); // None -> defaults
        assert!(info.size_bytes > 0);
        assert!(backup_dir
            .join(&info.id)
            .join("backup_metadata.json")
            .exists());
    }
}

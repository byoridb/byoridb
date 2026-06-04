// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Write-Ahead Log implementation
//!
//! WAL provides durability by writing operations to a log before applying them.
//! On crash recovery, the WAL is replayed to restore the database state.

use crate::error::{KVStoreError, Result};
use parking_lot::Mutex;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// WAL entry operation types
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OpType {
    Put = 1,
    Delete = 2,
}

impl TryFrom<u8> for OpType {
    type Error = KVStoreError;

    fn try_from(value: u8) -> Result<Self> {
        match value {
            1 => Ok(OpType::Put),
            2 => Ok(OpType::Delete),
            _ => Err(KVStoreError::Wal(format!("Invalid op type: {}", value))),
        }
    }
}

/// A single WAL entry
#[derive(Debug, Clone)]
pub struct WalEntry {
    pub lsn: u64,
    pub op: OpType,
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

impl WalEntry {
    /// Serialize entry to bytes
    /// Format: [lsn:8][op:1][key_len:4][key:N][value_len:4][value:M][checksum:4]
    pub fn serialize(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(8 + 1 + 4 + self.key.len() + 4 + self.value.len() + 4);

        buf.extend_from_slice(&self.lsn.to_le_bytes());
        buf.push(self.op as u8);
        buf.extend_from_slice(&(self.key.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.key);
        buf.extend_from_slice(&(self.value.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.value);

        // CRC32C checksum over all preceding bytes
        let checksum = Self::compute_checksum(&buf);
        buf.extend_from_slice(&checksum.to_le_bytes());

        buf
    }

    /// Deserialize entry from bytes
    pub fn deserialize(data: &[u8]) -> Result<(Self, usize)> {
        if data.len() < 17 {
            return Err(KVStoreError::Wal("Entry too short".to_string()));
        }

        let mut pos = 0;

        // Read LSN
        let lsn = u64::from_le_bytes(data[pos..pos + 8].try_into().unwrap());
        pos += 8;

        // Read op type
        let op = OpType::try_from(data[pos])?;
        pos += 1;

        // Read key
        let key_len = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;

        if data.len() < pos + key_len + 4 {
            return Err(KVStoreError::Wal("Entry truncated at key".to_string()));
        }
        let key = data[pos..pos + key_len].to_vec();
        pos += key_len;

        // Read value
        let value_len = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;

        if data.len() < pos + value_len + 4 {
            return Err(KVStoreError::Wal("Entry truncated at value".to_string()));
        }
        let value = data[pos..pos + value_len].to_vec();
        pos += value_len;

        // Verify checksum
        let stored_checksum = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());
        let computed_checksum = Self::compute_checksum(&data[..pos]);

        if stored_checksum != computed_checksum {
            return Err(KVStoreError::Wal("Checksum mismatch".to_string()));
        }
        pos += 4;

        Ok((
            WalEntry {
                lsn,
                op,
                key,
                value,
            },
            pos,
        ))
    }

    fn compute_checksum(data: &[u8]) -> u32 {
        crc32fast::hash(data)
    }
}

/// Write-Ahead Log
pub struct WAL {
    dir: PathBuf,
    current_file: Mutex<Option<BufWriter<File>>>,
    current_lsn: AtomicU64,
    max_file_size: u64,
    current_file_size: AtomicU64,
    file_index: AtomicU64,
}

impl WAL {
    /// Create a new WAL in the specified directory
    pub fn new<P: AsRef<Path>>(dir: P, max_file_size: u64) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;

        let wal = WAL {
            dir,
            current_file: Mutex::new(None),
            current_lsn: AtomicU64::new(0),
            max_file_size,
            current_file_size: AtomicU64::new(0),
            file_index: AtomicU64::new(0),
        };

        // Find existing WAL files and determine starting LSN
        wal.init()?;

        Ok(wal)
    }

    /// Initialize WAL - find existing files and set starting LSN
    fn init(&self) -> Result<()> {
        let mut max_lsn = 0u64;
        let mut max_index = 0u64;

        if let Ok(entries) = fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with("wal_") && name.ends_with(".log") {
                        if let Ok(index) = name[4..name.len() - 4].parse::<u64>() {
                            max_index = max_index.max(index);

                            // Read file to find max LSN
                            if let Ok(entries) = self.read_file(&path) {
                                for entry in entries {
                                    max_lsn = max_lsn.max(entry.lsn);
                                }
                            }
                        }
                    }
                }
            }
        }

        self.current_lsn.store(max_lsn, Ordering::SeqCst);
        self.file_index.store(max_index, Ordering::SeqCst);

        Ok(())
    }

    /// Get the current WAL file path
    fn current_file_path(&self) -> PathBuf {
        self.dir.join(format!(
            "wal_{:08}.log",
            self.file_index.load(Ordering::SeqCst)
        ))
    }

    /// Ensure we have an open file for writing
    fn ensure_file(&self) -> Result<()> {
        let mut file_guard = self.current_file.lock();

        if file_guard.is_none()
            || self.current_file_size.load(Ordering::SeqCst) >= self.max_file_size
        {
            // Close current file if exists
            if let Some(mut writer) = file_guard.take() {
                writer.flush()?;
            }

            // Increment file index for new file
            if self.current_file_size.load(Ordering::SeqCst) >= self.max_file_size {
                self.file_index.fetch_add(1, Ordering::SeqCst);
                self.current_file_size.store(0, Ordering::SeqCst);
            }

            // Open new file
            let path = self.current_file_path();
            let file = OpenOptions::new().create(true).append(true).open(&path)?;

            *file_guard = Some(BufWriter::new(file));
        }

        Ok(())
    }

    /// Append a PUT operation to the WAL.
    ///
    /// The entry is staged in the current file's `BufWriter`; it is only
    /// guaranteed on disk after the buffer fills, the file rotates, the
    /// caller invokes [`WAL::sync`], or [`WAL::append_batch`] runs (each
    /// batch flushes once). Single-entry callers that need durability
    /// must drive their own commit policy via `sync()`.
    pub fn append_put(&self, key: &[u8], value: &[u8]) -> Result<u64> {
        let lsn = self.current_lsn.fetch_add(1, Ordering::SeqCst) + 1;

        let entry = WalEntry {
            lsn,
            op: OpType::Put,
            key: key.to_vec(),
            value: value.to_vec(),
        };

        self.write_entry(&entry)?;
        Ok(lsn)
    }

    /// Append a DELETE operation to the WAL.
    ///
    /// Same buffering semantics as [`WAL::append_put`].
    pub fn append_delete(&self, key: &[u8]) -> Result<u64> {
        let lsn = self.current_lsn.fetch_add(1, Ordering::SeqCst) + 1;

        let entry = WalEntry {
            lsn,
            op: OpType::Delete,
            key: key.to_vec(),
            value: Vec::new(),
        };

        self.write_entry(&entry)?;
        Ok(lsn)
    }

    /// Append multiple PUT operations to the WAL in a single batch
    /// This is more efficient than calling append_put multiple times
    /// as it only performs one flush at the end
    pub fn append_batch(&self, entries: &[(OpType, Vec<u8>, Vec<u8>)]) -> Result<Vec<u64>> {
        if entries.is_empty() {
            return Ok(Vec::new());
        }

        self.ensure_file()?;

        let mut lsns = Vec::with_capacity(entries.len());
        let mut total_data = Vec::new();

        // Serialize all entries into a single buffer
        for (op, key, value) in entries {
            let lsn = self.current_lsn.fetch_add(1, Ordering::SeqCst) + 1;
            lsns.push(lsn);

            let entry = WalEntry {
                lsn,
                op: *op,
                key: key.clone(),
                value: value.clone(),
            };
            total_data.extend(entry.serialize());
        }

        let total_len = total_data.len() as u64;

        // Write all entries in one operation
        let mut file_guard = self.current_file.lock();
        if let Some(ref mut writer) = *file_guard {
            writer.write_all(&total_data)?;
            writer.flush()?; // Single flush for entire batch
        }

        self.current_file_size
            .fetch_add(total_len, Ordering::SeqCst);
        Ok(lsns)
    }

    /// Write an entry to the WAL.
    ///
    /// Bytes are written into the current file's `BufWriter`. We deliberately
    /// do *not* flush per write: the buffer holds entries until it fills, the
    /// file rotates, or the caller invokes [`WAL::sync`]. `append_batch` keeps
    /// its own explicit flush so a batch is still a single commit point.
    fn write_entry(&self, entry: &WalEntry) -> Result<()> {
        self.ensure_file()?;

        let data = entry.serialize();
        let data_len = data.len() as u64;

        let mut file_guard = self.current_file.lock();
        if let Some(ref mut writer) = *file_guard {
            writer.write_all(&data)?;
        }

        self.current_file_size.fetch_add(data_len, Ordering::SeqCst);
        Ok(())
    }

    /// Sync WAL to disk
    pub fn sync(&self) -> Result<()> {
        let mut file_guard = self.current_file.lock();
        if let Some(ref mut writer) = *file_guard {
            writer.flush()?;
            writer.get_ref().sync_all()?;
        }
        Ok(())
    }

    /// Read all entries from a WAL file
    fn read_file(&self, path: &Path) -> Result<Vec<WalEntry>> {
        let file = File::open(path)?;
        let mut reader = BufReader::new(file);
        let mut data = Vec::new();
        reader.read_to_end(&mut data)?;

        let mut entries = Vec::new();
        let mut pos = 0;

        while pos < data.len() {
            match WalEntry::deserialize(&data[pos..]) {
                Ok((entry, consumed)) => {
                    entries.push(entry);
                    pos += consumed;
                }
                Err(_) => break, // Stop on corrupted entry
            }
        }

        Ok(entries)
    }

    /// Recover all entries from WAL files (for replay)
    pub fn recover(&self) -> Result<Vec<WalEntry>> {
        let mut all_entries = Vec::new();

        // Get all WAL files sorted by index
        let mut files: Vec<PathBuf> = Vec::new();
        if let Ok(entries) = fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with("wal_") && name.ends_with(".log") {
                        files.push(path);
                    }
                }
            }
        }
        files.sort();

        // Read all entries
        for file_path in files {
            if let Ok(entries) = self.read_file(&file_path) {
                all_entries.extend(entries);
            }
        }

        // Sort by LSN
        all_entries.sort_by_key(|e| e.lsn);

        Ok(all_entries)
    }

    /// Clean old WAL files (keep only files after the given LSN)
    pub fn cleanup(&self, min_lsn: u64) -> Result<()> {
        if let Ok(entries) = fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with("wal_") && name.ends_with(".log") {
                        // Check if all entries in this file are below min_lsn
                        if let Ok(file_entries) = self.read_file(&path) {
                            if file_entries.iter().all(|e| e.lsn < min_lsn) {
                                let _ = fs::remove_file(&path);
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Get current LSN
    pub fn current_lsn(&self) -> u64 {
        self.current_lsn.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_wal_entry_serialize_deserialize() {
        let entry = WalEntry {
            lsn: 42,
            op: OpType::Put,
            key: b"test_key".to_vec(),
            value: b"test_value".to_vec(),
        };

        let serialized = entry.serialize();
        let (deserialized, _) = WalEntry::deserialize(&serialized).unwrap();

        assert_eq!(deserialized.lsn, 42);
        assert_eq!(deserialized.op, OpType::Put);
        assert_eq!(deserialized.key, b"test_key");
        assert_eq!(deserialized.value, b"test_value");
    }

    #[test]
    fn test_wal_append_and_recover() {
        let dir = tempdir().unwrap();
        let wal = WAL::new(dir.path(), 1024 * 1024).unwrap();

        // Append some entries
        wal.append_put(b"key1", b"value1").unwrap();
        wal.append_put(b"key2", b"value2").unwrap();
        wal.append_delete(b"key1").unwrap();
        // Single appends are buffered; force a commit point before reading back.
        wal.sync().unwrap();

        // Recover entries
        let entries = wal.recover().unwrap();

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].op, OpType::Put);
        assert_eq!(entries[0].key, b"key1");
        assert_eq!(entries[1].op, OpType::Put);
        assert_eq!(entries[1].key, b"key2");
        assert_eq!(entries[2].op, OpType::Delete);
        assert_eq!(entries[2].key, b"key1");
    }

    // ----- OpType ---------------------------------------------------------

    #[test]
    fn test_optype_try_from_valid() {
        assert_eq!(OpType::try_from(1u8).unwrap(), OpType::Put);
        assert_eq!(OpType::try_from(2u8).unwrap(), OpType::Delete);
    }

    #[test]
    fn test_optype_try_from_invalid_returns_wal_error() {
        // 0 is not a valid op (Put=1, Delete=2)
        let err = OpType::try_from(0u8).unwrap_err();
        assert!(matches!(err, KVStoreError::Wal(_)));

        // Anything outside 1..=2 should fail
        for byte in [3u8, 10, 99, 255] {
            let err = OpType::try_from(byte).unwrap_err();
            match err {
                KVStoreError::Wal(msg) => assert!(msg.contains(&byte.to_string())),
                other => panic!("expected Wal error, got {:?}", other),
            }
        }
    }

    // ----- WalEntry serialize/deserialize edges --------------------------

    #[test]
    fn test_wal_entry_empty_key_and_value() {
        // Delete entries have empty value; serialize/deserialize must round-trip
        let entry = WalEntry {
            lsn: 1,
            op: OpType::Delete,
            key: Vec::new(),
            value: Vec::new(),
        };
        let bytes = entry.serialize();
        let (decoded, consumed) = WalEntry::deserialize(&bytes).unwrap();
        assert_eq!(consumed, bytes.len());
        assert!(decoded.key.is_empty());
        assert!(decoded.value.is_empty());
        assert_eq!(decoded.op, OpType::Delete);
    }

    #[test]
    fn test_wal_entry_deserialize_too_short_header() {
        // Header alone needs >= 17 bytes (lsn 8 + op 1 + key_len 4 + value_len 4)
        let err = WalEntry::deserialize(&[]).unwrap_err();
        assert!(matches!(err, KVStoreError::Wal(_)));

        let err = WalEntry::deserialize(&[0u8; 16]).unwrap_err();
        assert!(matches!(err, KVStoreError::Wal(_)));
    }

    #[test]
    fn test_wal_entry_deserialize_truncated_at_key() {
        let entry = WalEntry {
            lsn: 7,
            op: OpType::Put,
            key: b"abcdef".to_vec(),
            value: b"x".to_vec(),
        };
        let bytes = entry.serialize();
        // Cut off well before the key body completes
        let truncated = &bytes[..15];
        let err = WalEntry::deserialize(truncated).unwrap_err();
        match err {
            KVStoreError::Wal(msg) => assert!(msg.contains("key") || msg.contains("short")),
            other => panic!("expected Wal error, got {:?}", other),
        }
    }

    #[test]
    fn test_wal_entry_deserialize_truncated_at_value() {
        let entry = WalEntry {
            lsn: 3,
            op: OpType::Put,
            key: b"k".to_vec(),
            value: b"hello world".to_vec(),
        };
        let bytes = entry.serialize();
        // Stop right after we've consumed the key but before the value finishes.
        // Header+key = 8 + 1 + 4 + 1 + 4 = 18 bytes; cut a few value bytes short.
        let truncated = &bytes[..bytes.len() - 6];
        let err = WalEntry::deserialize(truncated).unwrap_err();
        match err {
            KVStoreError::Wal(msg) => assert!(msg.contains("value") || msg.contains("truncated")),
            other => panic!("expected Wal error, got {:?}", other),
        }
    }

    #[test]
    fn test_wal_entry_deserialize_checksum_mismatch() {
        let entry = WalEntry {
            lsn: 99,
            op: OpType::Put,
            key: b"k".to_vec(),
            value: b"v".to_vec(),
        };
        let mut bytes = entry.serialize();
        // Layout: [lsn:8][op:1][key_len:4][key:1][value_len:4][value:1][cksum:4]
        // Index 13 is the key byte itself — flipping it changes the data
        // (and thus the recomputed checksum) without corrupting structural
        // length fields, so we hit the checksum-mismatch path.
        bytes[13] ^= 0xFF;
        let err = WalEntry::deserialize(&bytes).unwrap_err();
        match err {
            KVStoreError::Wal(msg) => assert!(
                msg.to_lowercase().contains("checksum"),
                "expected checksum error, got: {}",
                msg
            ),
            other => panic!("expected Wal checksum error, got {:?}", other),
        }
    }

    #[test]
    fn test_wal_entry_deserialize_consumed_bytes() {
        // When two entries are concatenated, deserialize must report the exact
        // length of the first so the caller can advance past it.
        let a = WalEntry {
            lsn: 1,
            op: OpType::Put,
            key: b"a".to_vec(),
            value: b"aa".to_vec(),
        };
        let b = WalEntry {
            lsn: 2,
            op: OpType::Delete,
            key: b"bb".to_vec(),
            value: Vec::new(),
        };
        let mut buf = a.serialize();
        let a_len = buf.len();
        buf.extend_from_slice(&b.serialize());

        let (first, consumed) = WalEntry::deserialize(&buf).unwrap();
        assert_eq!(consumed, a_len);
        assert_eq!(first.lsn, 1);

        let (second, _) = WalEntry::deserialize(&buf[consumed..]).unwrap();
        assert_eq!(second.lsn, 2);
        assert_eq!(second.op, OpType::Delete);
    }

    // ----- WAL: file rotation, LSN, batch, cleanup ----------------------

    #[test]
    fn test_wal_append_batch_empty_is_noop() {
        let dir = tempdir().unwrap();
        let wal = WAL::new(dir.path(), 1024).unwrap();
        let lsns = wal.append_batch(&[]).unwrap();
        assert!(lsns.is_empty());
        assert_eq!(wal.current_lsn(), 0);
        // No file should be needed yet
        let entries = wal.recover().unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_wal_append_batch_assigns_sequential_lsns() {
        let dir = tempdir().unwrap();
        let wal = WAL::new(dir.path(), 1024 * 1024).unwrap();
        let batch = vec![
            (OpType::Put, b"k1".to_vec(), b"v1".to_vec()),
            (OpType::Put, b"k2".to_vec(), b"v2".to_vec()),
            (OpType::Delete, b"k1".to_vec(), Vec::new()),
        ];
        let lsns = wal.append_batch(&batch).unwrap();
        assert_eq!(lsns, vec![1, 2, 3]);
        assert_eq!(wal.current_lsn(), 3);

        let entries = wal.recover().unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[2].op, OpType::Delete);
    }

    #[test]
    fn test_wal_file_rotation_on_size_limit() {
        let dir = tempdir().unwrap();
        // Tiny limit forces rotation after the very first entry.
        let wal = WAL::new(dir.path(), 32).unwrap();

        for i in 0..5u8 {
            wal.append_put(&[i], &[i; 16]).unwrap();
        }
        // Flush the trailing entry that hasn't triggered a rotation yet.
        wal.sync().unwrap();

        // Multiple wal_*.log files should now exist.
        let mut log_files: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter_map(|e| {
                let n = e.file_name().to_string_lossy().to_string();
                if n.starts_with("wal_") && n.ends_with(".log") {
                    Some(n)
                } else {
                    None
                }
            })
            .collect();
        log_files.sort();
        assert!(
            log_files.len() >= 2,
            "expected rotation but got {:?}",
            log_files
        );

        // recover() must merge them and preserve LSN order.
        let entries = wal.recover().unwrap();
        assert_eq!(entries.len(), 5);
        let lsns: Vec<u64> = entries.iter().map(|e| e.lsn).collect();
        assert_eq!(lsns, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_wal_lsn_persists_across_reopen() {
        let dir = tempdir().unwrap();
        {
            let wal = WAL::new(dir.path(), 1024 * 1024).unwrap();
            wal.append_put(b"k1", b"v1").unwrap();
            wal.append_put(b"k2", b"v2").unwrap();
            assert_eq!(wal.current_lsn(), 2);
            wal.sync().unwrap();
        }

        // Reopen and ensure LSN counter resumes after the highest existing LSN
        let wal2 = WAL::new(dir.path(), 1024 * 1024).unwrap();
        assert_eq!(wal2.current_lsn(), 2, "init() must restore max LSN");

        let lsn = wal2.append_put(b"k3", b"v3").unwrap();
        assert_eq!(lsn, 3, "next append must produce LSN 3, not 1");
        wal2.sync().unwrap();

        let entries = wal2.recover().unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries.last().unwrap().lsn, 3);
    }

    #[test]
    fn test_wal_cleanup_removes_only_old_files() {
        let dir = tempdir().unwrap();
        // Force rotation: tiny max_file_size means each entry lands in its own file
        let wal = WAL::new(dir.path(), 16).unwrap();
        for i in 0..4u8 {
            wal.append_put(&[i], &[0u8; 8]).unwrap();
        }
        wal.sync().unwrap();
        let total_before = fs::read_dir(dir.path()).unwrap().count();
        assert!(total_before >= 2);

        // Cleanup with min_lsn = 3 should drop files whose entries are all < 3
        wal.cleanup(3).unwrap();

        // After cleanup, replaying should still surface entries with LSN >= 3
        let entries = wal.recover().unwrap();
        assert!(
            entries.iter().any(|e| e.lsn >= 3),
            "post-cleanup recovery missing recent entries: {:?}",
            entries.iter().map(|e| e.lsn).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_wal_cleanup_no_op_when_min_lsn_zero() {
        let dir = tempdir().unwrap();
        let wal = WAL::new(dir.path(), 1024 * 1024).unwrap();
        wal.append_put(b"k", b"v").unwrap();
        wal.sync().unwrap();
        wal.cleanup(0).unwrap(); // min_lsn=0 means nothing is "old"
        let entries = wal.recover().unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn test_wal_sync_on_empty_log_is_ok() {
        // Calling sync() before any append must not panic or error.
        let dir = tempdir().unwrap();
        let wal = WAL::new(dir.path(), 1024).unwrap();
        wal.sync().unwrap();
    }

    #[test]
    fn test_wal_recover_skips_corrupted_tail() {
        // Write two valid entries, then truncate one byte in the second so
        // its checksum fails. recover() must return the first entry and stop.
        let dir = tempdir().unwrap();
        let log_path = {
            let wal = WAL::new(dir.path(), 1024 * 1024).unwrap();
            wal.append_put(b"good", b"1").unwrap();
            wal.append_put(b"bad", b"2").unwrap();
            wal.sync().unwrap();
            // Find the on-disk file
            fs::read_dir(dir.path())
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .find(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("wal_") && n.ends_with(".log"))
                })
                .unwrap()
        };

        // Corrupt the last byte (part of the checksum of entry #2)
        let mut data = fs::read(&log_path).unwrap();
        let last = data.len() - 1;
        data[last] ^= 0xFF;
        fs::write(&log_path, &data).unwrap();

        let wal2 = WAL::new(dir.path(), 1024 * 1024).unwrap();
        let entries = wal2.recover().unwrap();
        // First entry must survive; corrupted second entry is dropped.
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, b"good");
    }

    #[test]
    fn test_wal_handles_large_payload() {
        // 256 KiB value — exercises u32 length encoding and buffered writer
        let dir = tempdir().unwrap();
        let wal = WAL::new(dir.path(), 4 * 1024 * 1024).unwrap();
        let big = vec![0xABu8; 256 * 1024];
        wal.append_put(b"big", &big).unwrap();
        wal.sync().unwrap();

        let entries = wal.recover().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].value.len(), big.len());
        assert_eq!(entries[0].value, big);
    }

    #[test]
    fn test_wal_init_ignores_non_wal_files() {
        // Files that don't match `wal_<index>.log` must not break init()
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("README.md"), b"hello").unwrap();
        fs::write(dir.path().join("wal_oops.log"), b"garbage").unwrap(); // bad index
        fs::write(dir.path().join("notes.txt"), b"x").unwrap();

        let wal = WAL::new(dir.path(), 1024).unwrap();
        // No real WAL entries yet -> LSN starts at 0
        assert_eq!(wal.current_lsn(), 0);
        let lsn = wal.append_put(b"k", b"v").unwrap();
        assert_eq!(lsn, 1);
    }
}

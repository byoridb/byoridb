// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Raft log management
//!
//! This module provides persistent log storage for Raft consensus.

use super::types::{Command, LogEntry, LogIndex, Term};
use byoridb_kvstore::KVStore;
use std::sync::Arc;
use tracing::debug;

/// In-memory log storage (can be backed by KVStore for persistence)
#[allow(dead_code)]
pub struct RaftLog {
    /// Log entries (0-indexed, but log index is 1-based)
    entries: Vec<LogEntry>,
    /// First index in the log (after compaction)
    first_index: LogIndex,
    /// Last term before first_index (for snapshot)
    snapshot_term: Term,
    /// Optional KVStore for persistence
    kvstore: Option<Arc<dyn KVStore>>,
    /// Space and partition ID for this log
    space_id: u32,
    part_id: u32,
}

impl RaftLog {
    /// Create a new in-memory log
    pub fn new(space_id: u32, part_id: u32) -> Self {
        Self {
            entries: Vec::new(),
            first_index: 1,
            snapshot_term: 0,
            kvstore: None,
            space_id,
            part_id,
        }
    }

    /// Create a log backed by KVStore
    pub fn with_kvstore(space_id: u32, part_id: u32, kvstore: Arc<dyn KVStore>) -> Self {
        Self {
            entries: Vec::new(),
            first_index: 1,
            snapshot_term: 0,
            kvstore: Some(kvstore),
            space_id,
            part_id,
        }
    }

    /// Get the first log index
    pub fn first_index(&self) -> LogIndex {
        self.first_index
    }

    /// Get the last log index
    pub fn last_index(&self) -> LogIndex {
        if self.entries.is_empty() {
            self.first_index.saturating_sub(1)
        } else {
            self.first_index + self.entries.len() as LogIndex - 1
        }
    }

    /// Get the term of the last log entry
    pub fn last_term(&self) -> Term {
        self.entries
            .last()
            .map(|e| e.term)
            .unwrap_or(self.snapshot_term)
    }

    /// Get a log entry by index
    pub fn get(&self, index: LogIndex) -> Option<&LogEntry> {
        if index < self.first_index || index > self.last_index() {
            return None;
        }
        let offset = (index - self.first_index) as usize;
        self.entries.get(offset)
    }

    /// Get the term of a log entry
    pub fn term(&self, index: LogIndex) -> Option<Term> {
        if index == 0 {
            return Some(0);
        }
        if index < self.first_index {
            if index == self.first_index - 1 {
                return Some(self.snapshot_term);
            }
            return None;
        }
        self.get(index).map(|e| e.term)
    }

    /// Append entries to the log
    pub fn append(&mut self, entries: Vec<LogEntry>) {
        for entry in entries {
            debug!(
                "Appending log entry: index={}, term={}",
                entry.index, entry.term
            );
            self.entries.push(entry);
        }
    }

    /// Append a single entry and return its index
    pub fn append_entry(&mut self, term: Term, command: Command) -> LogIndex {
        let index = self.last_index() + 1;
        let entry = LogEntry {
            term,
            index,
            command,
        };
        self.entries.push(entry);
        index
    }

    /// Truncate the log from the given index (exclusive)
    pub fn truncate(&mut self, from_index: LogIndex) {
        if from_index <= self.first_index {
            self.entries.clear();
        } else if from_index <= self.last_index() {
            let offset = (from_index - self.first_index) as usize;
            self.entries.truncate(offset);
        }
    }

    /// Get entries from start_index (inclusive)
    pub fn entries_from(&self, start_index: LogIndex) -> Vec<LogEntry> {
        if start_index > self.last_index() {
            return Vec::new();
        }
        let start = if start_index < self.first_index {
            0
        } else {
            (start_index - self.first_index) as usize
        };
        self.entries[start..].to_vec()
    }

    /// Get entries in a range [start, end)
    pub fn entries_range(&self, start: LogIndex, end: LogIndex) -> Vec<LogEntry> {
        if start > self.last_index() || end <= start {
            return Vec::new();
        }
        let start_offset = if start < self.first_index {
            0
        } else {
            (start - self.first_index) as usize
        };
        let end_offset = if end > self.last_index() + 1 {
            self.entries.len()
        } else {
            (end - self.first_index) as usize
        };
        self.entries[start_offset..end_offset].to_vec()
    }

    /// Compact the log up to (and including) the given index
    pub fn compact(&mut self, up_to_index: LogIndex, last_term: Term) {
        if up_to_index < self.first_index {
            return;
        }

        let compact_count = (up_to_index - self.first_index + 1) as usize;
        if compact_count >= self.entries.len() {
            self.entries.clear();
        } else {
            self.entries.drain(0..compact_count);
        }

        self.first_index = up_to_index + 1;
        self.snapshot_term = last_term;

        debug!(
            "Compacted log up to index {}, new first_index={}",
            up_to_index, self.first_index
        );
    }

    /// Check if the log is up-to-date compared to candidate's log
    pub fn is_up_to_date(&self, last_log_index: LogIndex, last_log_term: Term) -> bool {
        let my_last_term = self.last_term();
        let my_last_index = self.last_index();

        if last_log_term != my_last_term {
            last_log_term > my_last_term
        } else {
            last_log_index >= my_last_index
        }
    }

    /// Find the conflict index and term when AppendEntries fails
    pub fn find_conflict(&self, index: LogIndex, term: Term) -> (LogIndex, Term) {
        if let Some(entry) = self.get(index) {
            if entry.term != term {
                // Find the first index of the conflicting term
                let conflict_term = entry.term;
                let mut conflict_index = index;
                while conflict_index > self.first_index {
                    if let Some(prev) = self.get(conflict_index - 1) {
                        if prev.term != conflict_term {
                            break;
                        }
                        conflict_index -= 1;
                    } else {
                        break;
                    }
                }
                return (conflict_index, conflict_term);
            }
        }
        (index, term)
    }

    /// Get log size (number of entries)
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if log is empty
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(index: LogIndex, term: Term) -> LogEntry {
        LogEntry {
            term,
            index,
            command: Command::Noop,
        }
    }

    #[test]
    fn test_empty_log() {
        let log = RaftLog::new(1, 1);
        assert_eq!(log.first_index(), 1);
        assert_eq!(log.last_index(), 0);
        assert_eq!(log.last_term(), 0);
        assert!(log.is_empty());
    }

    #[test]
    fn test_append_and_get() {
        let mut log = RaftLog::new(1, 1);

        log.append(vec![make_entry(1, 1), make_entry(2, 1), make_entry(3, 2)]);

        assert_eq!(log.len(), 3);
        assert_eq!(log.last_index(), 3);
        assert_eq!(log.last_term(), 2);

        assert!(log.get(0).is_none());
        assert_eq!(log.get(1).unwrap().term, 1);
        assert_eq!(log.get(2).unwrap().term, 1);
        assert_eq!(log.get(3).unwrap().term, 2);
        assert!(log.get(4).is_none());
    }

    #[test]
    fn test_truncate() {
        let mut log = RaftLog::new(1, 1);

        log.append(vec![make_entry(1, 1), make_entry(2, 1), make_entry(3, 2)]);

        log.truncate(2);
        assert_eq!(log.len(), 1);
        assert_eq!(log.last_index(), 1);
    }

    #[test]
    fn test_compact() {
        let mut log = RaftLog::new(1, 1);

        log.append(vec![
            make_entry(1, 1),
            make_entry(2, 1),
            make_entry(3, 2),
            make_entry(4, 2),
        ]);

        log.compact(2, 1);
        assert_eq!(log.first_index(), 3);
        assert_eq!(log.len(), 2);
        assert_eq!(log.term(2).unwrap(), 1); // snapshot term
        assert_eq!(log.get(3).unwrap().term, 2);
    }

    #[test]
    fn test_entries_from() {
        let mut log = RaftLog::new(1, 1);

        log.append(vec![make_entry(1, 1), make_entry(2, 1), make_entry(3, 2)]);

        let entries = log.entries_from(2);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].index, 2);
        assert_eq!(entries[1].index, 3);
    }

    #[test]
    fn test_is_up_to_date() {
        let mut log = RaftLog::new(1, 1);

        log.append(vec![make_entry(1, 1), make_entry(2, 2)]);

        // Same log
        assert!(log.is_up_to_date(2, 2));

        // Higher term
        assert!(log.is_up_to_date(1, 3));

        // Same term, higher index
        assert!(log.is_up_to_date(3, 2));

        // Lower term
        assert!(!log.is_up_to_date(2, 1));

        // Same term, lower index
        assert!(!log.is_up_to_date(1, 2));
    }
}

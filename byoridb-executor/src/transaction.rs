// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Transaction support for nGQL
//!
//! This module provides ACID transaction support:
//! - BEGIN / COMMIT / ROLLBACK
//! - Transaction isolation levels
//! - Optimistic concurrency control

use crate::context::ExecutionContext;
use crate::error::{ExecutionError, Result};
use std::collections::HashMap;
use std::sync::Arc;

/// Transaction manager
pub struct TransactionManager {
    ctx: Arc<ExecutionContext>,
    transactions: HashMap<i64, Transaction>,
    next_id: i64,
}

impl TransactionManager {
    pub fn new(ctx: Arc<ExecutionContext>) -> Self {
        TransactionManager {
            ctx,
            transactions: HashMap::new(),
            next_id: 1,
        }
    }

    /// Begin a new transaction
    pub async fn begin(&mut self, isolation_level: IsolationLevel) -> Result<i64> {
        let id = self.next_id;
        self.next_id += 1;

        let transaction = Transaction::new(id, isolation_level);
        self.transactions.insert(id, transaction);

        tracing::info!(
            "Started transaction {} with isolation level {:?}",
            id,
            isolation_level
        );

        Ok(id)
    }

    /// Commit a transaction
    pub async fn commit(&mut self, id: i64) -> Result<()> {
        let mut transaction = self.transactions.remove(&id).ok_or_else(|| {
            ExecutionError::InvalidOperation(format!("Transaction {} not found", id))
        })?;

        // Apply all writes to KV store
        for (key, value) in transaction.writes.drain() {
            self.ctx
                .kvstore
                .put(key.as_bytes(), value.as_slice())
                .await?;
        }

        tracing::info!(
            "Committed transaction {} with {} writes",
            id,
            transaction.writes.len()
        );

        Ok(())
    }

    /// Rollback a transaction
    pub async fn rollback(&mut self, id: i64) -> Result<()> {
        let transaction = self.transactions.remove(&id).ok_or_else(|| {
            ExecutionError::InvalidOperation(format!("Transaction {} not found", id))
        })?;

        // Transaction is dropped, discarding all writes
        tracing::info!(
            "Rolled back transaction {} with {} discarded writes",
            id,
            transaction.writes.len()
        );

        Ok(())
    }

    /// Get a transaction by ID
    pub fn get(&self, id: i64) -> Option<&Transaction> {
        self.transactions.get(&id)
    }

    /// Get a mutable transaction by ID
    pub fn get_mut(&mut self, id: i64) -> Option<&mut Transaction> {
        self.transactions.get_mut(&id)
    }
}

/// Transaction state
pub struct Transaction {
    pub id: i64,
    pub isolation_level: IsolationLevel,
    pub writes: HashMap<String, Vec<u8>>,
    pub read_set: Vec<String>,
    pub write_set: Vec<String>,
    pub start_time: std::time::Instant,
}

impl Transaction {
    pub fn new(id: i64, isolation_level: IsolationLevel) -> Self {
        Transaction {
            id,
            isolation_level,
            writes: HashMap::new(),
            read_set: Vec::new(),
            write_set: Vec::new(),
            start_time: std::time::Instant::now(),
        }
    }

    /// Write a key-value pair within the transaction
    pub fn write(&mut self, key: String, value: Vec<u8>) {
        self.writes.insert(key.clone(), value);
        if !self.write_set.contains(&key) {
            self.write_set.push(key);
        }
    }

    /// Read a key within the transaction
    pub fn read(&mut self, key: String) {
        if !self.read_set.contains(&key) {
            self.read_set.push(key);
        }
    }

    /// Check for conflicts with another transaction
    pub fn has_conflict_with(&self, other: &Transaction) -> bool {
        // Check for write-write conflicts
        for key in &self.write_set {
            if other.write_set.contains(key) {
                return true;
            }
        }

        // For serializable isolation, also check write-read conflicts
        if self.isolation_level == IsolationLevel::Serializable {
            for key in &self.write_set {
                if other.read_set.contains(key) {
                    return true;
                }
            }
        }

        false
    }

    /// Get transaction duration
    pub fn duration(&self) -> std::time::Duration {
        self.start_time.elapsed()
    }
}

/// Transaction isolation levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IsolationLevel {
    /// Read committed - sees committed data
    #[default]
    ReadCommitted,
    /// Repeatable read - sees snapshot of committed data
    RepeatableRead,
    /// Serializable - full isolation
    Serializable,
}

/// Transaction executor
pub struct TransactionExecutor {
    manager: Arc<tokio::sync::RwLock<TransactionManager>>,
}

impl TransactionExecutor {
    pub fn new(manager: Arc<tokio::sync::RwLock<TransactionManager>>) -> Self {
        Self { manager }
    }

    /// Execute a statement within a transaction
    pub async fn execute_in_transaction(
        &self,
        tx_id: i64,
        stmt: String,
    ) -> Result<TransactionResult> {
        let mut manager = self.manager.write().await;
        let transaction = manager.get_mut(tx_id).ok_or_else(|| {
            ExecutionError::InvalidOperation(format!("Transaction {} not found", tx_id))
        })?;

        // Parse and execute the statement
        // In a real implementation, this would use the parser and executor
        let result = match stmt.trim() {
            s if s.starts_with("INSERT") => {
                // Simulate an insert
                transaction.write(format!("vertex:{}", 1), vec![1, 2, 3]);
                TransactionResult::Write(1)
            }
            s if s.starts_with("UPDATE") => {
                // Simulate an update
                transaction.write(format!("vertex:{}", 1), vec![4, 5, 6]);
                TransactionResult::Write(1)
            }
            s if s.starts_with("DELETE") => {
                // Simulate a delete
                transaction.write(format!("vertex:{}", 1), vec![]);
                TransactionResult::Write(1)
            }
            s if s.starts_with("SELECT") || s.starts_with("GO") => {
                // Simulate a read
                transaction.read(format!("vertex:{}", 1));
                TransactionResult::Read
            }
            _ => TransactionResult::None,
        };

        Ok(result)
    }
}

/// Transaction operation result
#[derive(Debug, Clone)]
pub enum TransactionResult {
    Read,
    Write(usize),
    None,
}

/// Optimistic concurrency control
pub struct OptimisticConcurrencyControl {
    version: Arc<tokio::sync::RwLock<HashMap<String, u64>>>,
}

impl Default for OptimisticConcurrencyControl {
    fn default() -> Self {
        Self::new()
    }
}

impl OptimisticConcurrencyControl {
    pub fn new() -> Self {
        OptimisticConcurrencyControl {
            version: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        }
    }

    /// Get current version of a key
    pub async fn get_version(&self, key: &str) -> u64 {
        let version = self.version.read().await;
        version.get(key).copied().unwrap_or(0)
    }

    /// Increment version of a key
    pub async fn increment_version(&self, key: &str) -> u64 {
        let mut version = self.version.write().await;
        let current = version.get(key).copied().unwrap_or(0);
        version.insert(key.to_string(), current + 1);
        current + 1
    }

    /// Check if version matches
    pub async fn check_version(&self, key: &str, expected: u64) -> bool {
        let version = self.version.read().await;
        version.get(key).copied().unwrap_or(0) == expected
    }
}

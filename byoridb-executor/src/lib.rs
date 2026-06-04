// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Query execution engine for nGQL
//!
//! This crate provides the execution engine for parsed nGQL queries:
//! - DDL execution: CREATE, DROP, ALTER
//! - DML execution: INSERT, UPDATE, DELETE, UPSERT
//! - DQL execution: SELECT (GO, FETCH, LOOKUP, MATCH)
//! - Distributed query execution across multiple Storage nodes

pub mod algo;
pub mod arena;
pub mod context;
// Distributed execution (remote Storage via gRPC) requires tonic; gate it so
// the embedded local-only build stays wasm-capable.
#[cfg(feature = "distributed")]
pub mod distributed;
pub mod error;
pub mod evaluator;
pub mod executor;
pub mod explain;
pub mod key;
pub mod match_impl;
pub mod plan;
pub mod profile;
#[cfg(feature = "distributed")]
pub mod storage_client;
pub mod transaction;

pub use arena::{ArenaPool, QueryArena};
pub use context::{ExecutionConfig, ExecutionContext};
#[cfg(feature = "distributed")]
pub use distributed::{DistributedQueryConfig, DistributedQueryError, DistributedQueryExecutor};
pub use error::{ExecutionError, Result};
pub use executor::{Executor, ExecutorResult};
pub use key::SchemaKey;
pub use match_impl::{MatchExecutor, PatternMatcher};
pub use plan::{ExecutionPlan, ExecutionPlanBuilder};
pub use profile::{ProfileCollector, ProfileOp, ProfileRecord};
#[cfg(feature = "distributed")]
pub use storage_client::{StorageClientError, StorageQueryClient, StorageQueryClientConfig};
pub use transaction::{
    IsolationLevel, OptimisticConcurrencyControl, Transaction, TransactionExecutor,
    TransactionManager, TransactionResult,
};

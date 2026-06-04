// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Storage service for ByoriDB
//!
//! This module provides the storage layer that handles:
//! - Vertex and edge storage
//! - Query processing
//! - Index management
//! - Data partitioning
//! - Partition migration and management

pub mod codec;
// `env` (StorageEnv) opens a redb-backed KVStore — pure Rust, always available
// (embedded or server). `server` (StorageServer) serves it over gRPC and is
// gated behind `distributed`.
pub mod env;
pub mod error;
// The heartbeat sender talks to the Meta cluster via MetaClient (gRPC), so it
// is distributed-only.
#[cfg(feature = "distributed")]
pub mod heartbeat;
pub mod index;
pub mod key;
pub mod partition;
pub mod processor;
pub mod raft;
// The gRPC RPC layer (and its generated proto types) require tonic, which is
// not wasm-compatible. Gate behind `distributed`; embedded never serves RPC.
#[cfg(feature = "distributed")]
pub mod rpc;
#[cfg(feature = "distributed")]
pub mod server;

/// Generated protobuf types
#[cfg(feature = "distributed")]
pub mod proto {
    pub mod raft {
        tonic::include_proto!("raft");
    }
    pub mod storage {
        tonic::include_proto!("storage");
    }
}

pub use error::{Result, StorageError};
#[cfg(feature = "distributed")]
pub use heartbeat::{spawn_heartbeat_sender, HeartbeatConfig, HeartbeatSender};
pub use index::{
    EdgeIndexScanResult, IndexDef, IndexError, IndexManager, IndexType, ScanOptions,
    TagIndexScanResult,
};
pub use key::{IndexValue, KeyType, KeyUtils};
pub use partition::{PartitionError, PartitionInfo, PartitionManager, PartitionStatus};
pub use processor::StorageProcessor;
pub use raft::{
    AppendEntriesRequest, AppendEntriesResponse, ClusterConfig, Command, LogEntry, LogIndex,
    NodeId, NodeInfo, RaftAction, RaftConfig, RaftError, RaftGroupManager, RaftLog, RaftNode,
    RequestVoteRequest, RequestVoteResponse, Term,
};
#[cfg(feature = "distributed")]
pub use rpc::StorageRpcService;
#[cfg(feature = "distributed")]
pub use server::StorageServer;

// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Meta service for ByoriDB
//!
//! This module provides metadata management:
//! - Space management
//! - Schema management (Tag, Edge)
//! - Index management
//! - User and role management
//! - Host and partition management

// The gRPC layer — proto types, MetaClient, StorageClient, the rpc/service
// orchestrators and the rebalancer/allocator/failure-detector that drive a
// distributed cluster — all require tonic (not wasm-compatible). Gate them
// behind `distributed`. Embedded only needs the pure metadata types below.
#[cfg(feature = "distributed")]
pub mod allocator;
#[cfg(feature = "distributed")]
pub mod client;
pub mod error;
#[cfg(feature = "distributed")]
pub mod failure_detector;
pub mod hotspot;
pub mod key;
#[cfg(feature = "distributed")]
pub mod rebalancer;
#[cfg(feature = "distributed")]
pub mod rpc;
pub mod schema;
// `server` (MetaServer) opens a RocksdbKVStore — server-only, requires RocksDB.
#[cfg(feature = "rocksdb")]
pub mod server;
#[cfg(feature = "distributed")]
pub mod service;
#[cfg(feature = "distributed")]
pub mod storage_client;

/// Generated protobuf types and gRPC service definitions
#[cfg(feature = "distributed")]
pub mod proto {
    pub mod meta {
        tonic::include_proto!("meta");
    }
    pub mod storage {
        tonic::include_proto!("storage");
    }
    pub use meta::*;
}

#[cfg(feature = "distributed")]
pub use client::{HostInfo, HostLiveness, MetaClient};
pub use error::{MetaError, Result};
#[cfg(feature = "distributed")]
pub use failure_detector::{FailedNode, FailureDetector, FailureDetectorConfig, RecoveryResult};
#[cfg(feature = "rocksdb")]
pub use server::{MetaServer, MetaServerConfig};
#[cfg(feature = "distributed")]
pub use service::MetaService;
#[cfg(feature = "distributed")]
pub use storage_client::{MigrationResult, PartitionStatus, StorageClient};

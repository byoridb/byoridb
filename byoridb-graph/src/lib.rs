// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Graph service for ByoriDB
//!
//! This module provides the query execution engine:
//! - Query parsing and validation
//! - Execution planning
//! - Query optimization
//! - Result serialization

pub mod adapter;
pub mod auth;
pub mod context;
pub mod error;
pub mod evaluator;
pub mod executor;
// gRPC (tonic) and HTTP (axum) servers are server-only.
#[cfg(feature = "server")]
pub mod grpc;
pub mod logging;
pub mod metrics;
pub mod partition;
pub mod planner;
#[cfg(feature = "server")]
pub mod server;
pub mod service;
pub mod session;

pub use auth::{AuthManager, Permission, PermissionEntry, Role, Session, User};
pub use error::{GraphError, Result};
#[cfg(feature = "server")]
pub use logging::init_logging;
pub use logging::{LogConfig, QueryLogger};
#[cfg(feature = "server")]
pub use metrics::{init_metrics, render_metrics};
pub use metrics::{QueryTimer, QueryType};
pub use partition::{compute_partition, compute_partitions, PartitionRouter, SpacePartitionInfo};
#[cfg(feature = "server")]
pub use server::{GraphServer, HttpServer};
pub use service::GraphService;

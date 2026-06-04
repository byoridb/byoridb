// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Schema definitions for metadata

use serde::{Deserialize, Serialize};

/// Space (graph database) definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Space {
    pub id: u32,
    pub name: String,
    pub partition_num: u32,
    pub replica_factor: u32,
    pub vid_type: VidType,
    #[serde(default)]
    pub partition_strategy: byoridb_common::PartitionStrategy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum VidType {
    Int64,
    FixedString(usize),
}

/// Tag schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagSchema {
    pub id: u32,
    pub space_id: u32,
    pub name: String,
    pub version: i32,
    pub fields: Vec<Field>,
}

/// Edge schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeSchema {
    pub id: u32,
    pub space_id: u32,
    pub name: String,
    pub version: i32,
    pub fields: Vec<Field>,
}

/// Field definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    pub data_type: DataType,
    pub nullable: bool,
    pub default: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DataType {
    Bool,
    Int8,
    Int16,
    Int32,
    Int64,
    Float,
    Double,
    String,
    FixedString(usize),
    Timestamp,
    Date,
    Time,
    DateTime,
    Geography,
}

/// Tag index
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagIndex {
    pub id: u32,
    pub space_id: u32,
    pub index_name: String,
    pub tag_id: u32,
    pub fields: Vec<String>,
}

/// Edge index
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeIndex {
    pub id: u32,
    pub space_id: u32,
    pub index_name: String,
    pub edge_type: u32,
    pub fields: Vec<String>,
}

/// User definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub username: String,
    pub password_hash: String,
    pub role: Role,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Role {
    God,
    Admin,
    Dba,
    User,
    Guest,
}

/// Host information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Host {
    pub host: String,
    pub port: u32,
    pub status: HostStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum HostStatus {
    Online,
    Offline,
    Leader,
}

/// Partition allocation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartAllocation {
    pub space_id: u32,
    pub part_id: u32,
    pub hosts: Vec<(String, u32)>,
}

/// ALTER operation type for schema changes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AlterOperation {
    /// Add a new column to the schema
    AddColumn(Field),
    /// Drop an existing column by name
    DropColumn(String),
    /// Change an existing column's type/nullability/default
    ChangeColumn(Field),
}

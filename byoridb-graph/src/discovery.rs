//! Versioned, bounded schema discovery. It reads live metadata, never a snapshot.

use byoridb_executor::key::SchemaKey;
use byoridb_kvstore::KVStore;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DISCOVERY_CONTRACT_VERSION: &str = "1";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryRequest {
    pub contract_version: String,
    pub space: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct DiscoveryLimits {
    pub max_tags: u32,
    pub max_edges: u32,
    pub max_properties_per_type: u32,
    /// Cumulative stored key + value bytes read for the space, tags and edges.
    pub max_metadata_bytes: u32,
}

impl Default for DiscoveryLimits {
    fn default() -> Self {
        Self {
            max_tags: 256,
            max_edges: 256,
            max_properties_per_type: 128,
            max_metadata_bytes: 256 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct DiscoveryCapability {
    pub name: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct DiscoveredProperty {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub has_default: bool,
    pub max_utf8_bytes: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct DiscoveredType {
    pub name: String,
    pub properties: Vec<DiscoveredProperty>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct DiscoveredVid {
    pub kind: String,
    pub max_utf8_bytes: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct DiscoveredSchema {
    pub space: String,
    pub vid: DiscoveredVid,
    pub tags: Vec<DiscoveredType>,
    pub edges: Vec<DiscoveredType>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct DiscoveryResponse {
    pub contract_version: String,
    pub server_version: String,
    pub authorization_model: String,
    /// Effective built-in role permissions, not space-scoped grants.
    pub permissions: Vec<String>,
    pub capabilities: Vec<DiscoveryCapability>,
    pub schema_state: String,
    pub consistency: String,
    pub schema_snapshot: bool,
    pub schema_revision: bool,
    pub limits: DiscoveryLimits,
    pub schema: Option<DiscoveredSchema>,
}

impl DiscoveryResponse {
    pub(crate) fn new(permissions: Vec<String>, schema: Option<DiscoveredSchema>) -> Self {
        let mut capabilities: Vec<_> = ["schema_discovery", "text_query", "request_read_only"]
            .into_iter()
            .map(|name| DiscoveryCapability {
                name: name.into(),
                status: "available".into(),
            })
            .chain(
                [
                    "parameter_binding",
                    "snapshot_pagination",
                    "durable_idempotency",
                    "mutation_preview",
                    "revision_preconditions",
                    "space_scoped_grants",
                ]
                .into_iter()
                .map(|name| DiscoveryCapability {
                    name: name.into(),
                    status: "unsupported".into(),
                }),
            )
            .collect();
        capabilities.sort_by(|a, b| a.name.cmp(&b.name));
        Self {
            contract_version: DISCOVERY_CONTRACT_VERSION.into(),
            server_version: env!("CARGO_PKG_VERSION").into(),
            authorization_model: "built_in_global_roles".into(),
            permissions,
            capabilities,
            schema_state: if schema.is_some() {
                "available"
            } else {
                "not_selected"
            }
            .into(),
            consistency: "live_reads_no_snapshot".into(),
            schema_snapshot: false,
            schema_revision: false,
            limits: DiscoveryLimits::default(),
            schema,
        }
    }
}

/// Fixed public diagnostics: never serialize storage errors, names or bearer IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DiscoveryError {
    #[error("A session credential is required.")]
    AuthRequired,
    #[error("The session is absent or expired.")]
    SessionExpired,
    #[error("The session does not permit discovery.")]
    PermissionDenied,
    #[error("The discovery request is invalid.")]
    InvalidRequest,
    #[error("The discovery contract version is unsupported.")]
    UnsupportedContract,
    #[error("The requested space does not exist.")]
    SpaceNotFound,
    #[error("The discovery metadata exceeds the advertised limits.")]
    ResourceExhausted,
    #[error("The metadata store could not be read.")]
    StorageError,
    #[error("The stored schema metadata is invalid.")]
    InvalidMetadata,
    #[error("The service is not accepting requests.")]
    ServiceUnavailable,
}

impl DiscoveryError {
    pub fn code(self) -> &'static str {
        match self {
            Self::AuthRequired => "AUTH_REQUIRED",
            Self::SessionExpired => "SESSION_EXPIRED",
            Self::PermissionDenied => "PERMISSION_DENIED",
            Self::InvalidRequest => "INVALID_REQUEST",
            Self::UnsupportedContract => "UNSUPPORTED_CONTRACT",
            Self::SpaceNotFound => "SPACE_NOT_FOUND",
            Self::ResourceExhausted => "RESOURCE_EXHAUSTED",
            Self::StorageError => "STORAGE_ERROR",
            Self::InvalidMetadata => "INVALID_METADATA",
            Self::ServiceUnavailable => "SERVICE_UNAVAILABLE",
        }
    }
}

impl Serialize for DiscoveryError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut object = serializer.serialize_struct("DiscoveryError", 3)?;
        object.serialize_field("contract_version", DISCOVERY_CONTRACT_VERSION)?;
        object.serialize_field("code", self.code())?;
        object.serialize_field("message", &self.to_string())?;
        object.end()
    }
}

/// `:` delimits metadata namespaces. Reject controls and oversized names too.
pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 255 && !name.chars().any(|c| c == ':' || c.is_control())
}

fn metadata_name(value: &Value) -> Result<&str, DiscoveryError> {
    value
        .as_str()
        .filter(|name| valid_name(name))
        .ok_or(DiscoveryError::InvalidMetadata)
}

fn charge(remaining: &mut usize, key: &[u8], value: &[u8]) -> Result<(), DiscoveryError> {
    *remaining = remaining
        .checked_sub(key.len())
        .and_then(|n| n.checked_sub(value.len()))
        .ok_or(DiscoveryError::ResourceExhausted)?;
    Ok(())
}

fn fixed_string_size(text: &str, prefix: &str) -> Option<u32> {
    let n = text.strip_prefix(prefix)?.strip_suffix(')')?;
    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    n.parse::<u32>().ok().filter(|n| *n > 0)
}

fn property_type(value: &Value) -> Result<(String, Option<u32>), DiscoveryError> {
    if let Some(name) = value.as_str() {
        let normalized = match name {
            "Bool" => "bool",
            "Int8" => "int8",
            "Int16" => "int16",
            "Int32" => "int32",
            "Int64" => "int64",
            "Float" => "float",
            "Double" => "double",
            "String" => "string",
            "Timestamp" => "timestamp",
            "Date" => "date",
            "Time" => "time",
            "DateTime" => "datetime",
            "Geography" => "geography",
            other => {
                return fixed_string_size(other, "FixedString(")
                    .map(|n| ("fixed_string".into(), Some(n)))
                    .ok_or(DiscoveryError::InvalidMetadata)
            }
        };
        return Ok((normalized.into(), None));
    }
    // CREATE uses serde's enum representation; ALTER stores the Debug spelling.
    let object = value.as_object().ok_or(DiscoveryError::InvalidMetadata)?;
    let size = object
        .get("FixedString")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n > 0);
    if object.len() != 1 {
        return Err(DiscoveryError::InvalidMetadata);
    }
    size.map(|n| ("fixed_string".into(), Some(n)))
        .ok_or(DiscoveryError::InvalidMetadata)
}

async fn read_types(
    store: &dyn KVStore,
    prefix: &[u8],
    count: u32,
    properties_limit: u32,
    remaining: &mut usize,
) -> Result<Vec<DiscoveredType>, DiscoveryError> {
    let rows = store
        .scan_prefix_limited(prefix, Some(count as usize + 1))
        .await
        .map_err(|_| DiscoveryError::StorageError)?;
    if rows.len() > count as usize {
        return Err(DiscoveryError::ResourceExhausted);
    }
    let mut types = Vec::with_capacity(rows.len());
    for (key, bytes) in rows {
        charge(remaining, &key, &bytes)?;
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| DiscoveryError::InvalidMetadata)?;
        let name = metadata_name(&value["name"])?;
        let suffix = key
            .strip_prefix(prefix)
            .and_then(|suffix| std::str::from_utf8(suffix).ok());
        if suffix != Some(name) {
            return Err(DiscoveryError::InvalidMetadata);
        }
        let stored_properties = value["properties"]
            .as_array()
            .ok_or(DiscoveryError::InvalidMetadata)?;
        if stored_properties.len() > properties_limit as usize {
            return Err(DiscoveryError::ResourceExhausted);
        }
        let mut properties = Vec::with_capacity(stored_properties.len());
        for property in stored_properties {
            let name = metadata_name(&property["name"])?;
            let (data_type, max_utf8_bytes) = property_type(&property["data_type"])?;
            let nullable = property["nullable"]
                .as_bool()
                .ok_or(DiscoveryError::InvalidMetadata)?;
            let default = property
                .get("default_value")
                .ok_or(DiscoveryError::InvalidMetadata)?;
            if !default.is_null() {
                serde_json::from_value::<byoridb_parser::ast::Expression>(default.clone())
                    .map_err(|_| DiscoveryError::InvalidMetadata)?;
            }
            properties.push(DiscoveredProperty {
                name: name.into(),
                data_type,
                nullable,
                has_default: !default.is_null(),
                max_utf8_bytes,
            });
        }
        properties.sort_by(|a, b| a.name.cmp(&b.name));
        if properties.windows(2).any(|p| p[0].name == p[1].name) {
            return Err(DiscoveryError::InvalidMetadata);
        }
        types.push(DiscoveredType {
            name: name.into(),
            properties,
        });
    }
    types.sort_by(|a, b| a.name.cmp(&b.name));
    if types.windows(2).any(|t| t[0].name == t[1].name) {
        return Err(DiscoveryError::InvalidMetadata);
    }
    Ok(types)
}

pub(crate) async fn read_schema(
    store: &dyn KVStore,
    space: &str,
    limits: &DiscoveryLimits,
) -> Result<DiscoveredSchema, DiscoveryError> {
    let mut remaining = limits.max_metadata_bytes as usize;
    let key = SchemaKey::space(space);
    let bytes = store
        .get(&key)
        .await
        .map_err(|_| DiscoveryError::StorageError)?
        .ok_or(DiscoveryError::SpaceNotFound)?;
    charge(&mut remaining, &key, &bytes)?;
    let metadata: Value =
        serde_json::from_slice(&bytes).map_err(|_| DiscoveryError::InvalidMetadata)?;
    if metadata_name(&metadata["name"])? != space {
        return Err(DiscoveryError::InvalidMetadata);
    }
    let vid_type = metadata["vid_type"]
        .as_str()
        .ok_or(DiscoveryError::InvalidMetadata)?;
    let vid = if vid_type == "INT64" {
        DiscoveredVid {
            kind: "int64".into(),
            max_utf8_bytes: None,
        }
    } else {
        DiscoveredVid {
            kind: "fixed_string".into(),
            max_utf8_bytes: Some(
                fixed_string_size(vid_type, "FIXED_STRING(")
                    .ok_or(DiscoveryError::InvalidMetadata)?,
            ),
        }
    };
    let tags = read_types(
        store,
        &SchemaKey::tag_prefix(space),
        limits.max_tags,
        limits.max_properties_per_type,
        &mut remaining,
    )
    .await?;
    let edges = read_types(
        store,
        &SchemaKey::edge_prefix(space),
        limits.max_edges,
        limits.max_properties_per_type,
        &mut remaining,
    )
    .await?;
    Ok(DiscoveredSchema {
        space: space.into(),
        vid,
        tags,
        edges,
    })
}

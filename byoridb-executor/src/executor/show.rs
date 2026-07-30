// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use super::{Executor, ExecutorResult};
use crate::error::{ExecutionError, Result};
use crate::key::{SchemaKey, USER_KEY_PREFIX};

impl Executor {
    pub(super) async fn execute_show(&self, plan: crate::plan::ShowPlan) -> Result<ExecutorResult> {
        match plan {
            crate::plan::ShowPlan::Spaces => self.execute_show_spaces().await,
            crate::plan::ShowPlan::Tags => self.execute_show_tags().await,
            crate::plan::ShowPlan::Classes => self.execute_show_classes().await,
            crate::plan::ShowPlan::Edges => self.execute_show_edges().await,
            crate::plan::ShowPlan::TagIndexes => self.execute_show_tag_indexes().await,
            crate::plan::ShowPlan::EdgeIndexes => self.execute_show_edge_indexes().await,
            crate::plan::ShowPlan::Users => self.execute_show_users().await,
            crate::plan::ShowPlan::Parts => self.execute_show_parts().await,
            crate::plan::ShowPlan::Hosts => self.execute_show_hosts().await,
            crate::plan::ShowPlan::Stats => self.execute_show_stats().await,
            crate::plan::ShowPlan::Sessions => self.execute_show_sessions().await,
            crate::plan::ShowPlan::CreateTag(n) => self.execute_show_create_tag(&n).await,
            crate::plan::ShowPlan::CreateEdge(n) => self.execute_show_create_edge(&n).await,
            crate::plan::ShowPlan::TagIndexStatuses => self.execute_show_index_statuses(true).await,
            crate::plan::ShowPlan::EdgeIndexStatuses => {
                self.execute_show_index_statuses(false).await
            }
        }
    }

    /// Execute SHOW SPACES — lists all spaces from the meta service.
    pub(super) async fn execute_show_spaces(&self) -> Result<ExecutorResult> {
        let columns = vec![
            "ID".to_string(),
            "Name".to_string(),
            "Partition Num".to_string(),
            "Replica Factor".to_string(),
            "Vid Type".to_string(),
        ];

        #[cfg(feature = "distributed")]
        if let Some(client) = &self.ctx.meta_client {
            match client.list_spaces().await {
                Ok(spaces) => {
                    let rows = spaces
                        .into_iter()
                        .map(|s| {
                            vec![
                                byoridb_common::Value::Int(s.id as i64),
                                byoridb_common::Value::String(s.name),
                                byoridb_common::Value::Int(s.partition_num as i64),
                                byoridb_common::Value::Int(s.replica_factor as i64),
                                byoridb_common::Value::String(Self::format_vid_type(&s.vid_type)),
                            ]
                        })
                        .collect();
                    return Ok(ExecutorResult {
                        columns,
                        rows,
                        latency_ms: 0,
                    });
                }
                Err(e) => {
                    tracing::warn!("SHOW SPACES: failed to list spaces from meta: {}", e);
                }
            }
        }

        // Fallback: scan kvstore for space entries (for embedded/test configurations
        // that write schema directly to kvstore without a meta service).
        let rows = self.show_spaces_from_kvstore().await.unwrap_or_default();
        Ok(ExecutorResult {
            columns,
            rows,
            latency_ms: 0,
        })
    }

    /// Scan the kvstore for space entries when no meta client is configured.
    pub(super) async fn show_spaces_from_kvstore(&self) -> Result<Vec<Vec<byoridb_common::Value>>> {
        let prefix = SchemaKey::space_prefix();
        let entries = self.ctx.kvstore.scan_prefix(&prefix).await?;

        let mut rows = Vec::new();
        for (k, v) in entries {
            let key_str = match std::str::from_utf8(&k) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let name = match key_str.strip_prefix("space:") {
                Some(n) => n,
                None => continue,
            };
            // Skip nested keys like "space:<name>:tag:..."
            if name.contains(':') {
                continue;
            }

            let data: serde_json::Value =
                serde_json::from_slice(&v).unwrap_or(serde_json::Value::Null);
            let partition_num = data
                .get("partition_num")
                .and_then(|x| x.as_u64())
                .unwrap_or(0) as i64;
            let replica_factor = data
                .get("replica_factor")
                .and_then(|x| x.as_u64())
                .unwrap_or(0) as i64;
            let vid_type = data
                .get("vid_type")
                .and_then(|x| x.as_str())
                .unwrap_or("INT64")
                .to_string();

            let id = data.get("id").and_then(|x| x.as_u64()).unwrap_or(0) as i64;
            rows.push(vec![
                byoridb_common::Value::Int(id),
                byoridb_common::Value::String(name.to_string()),
                byoridb_common::Value::Int(partition_num),
                byoridb_common::Value::Int(replica_factor),
                byoridb_common::Value::String(vid_type),
            ]);
        }
        Ok(rows)
    }

    /// Execute SHOW TAGS — lists all tags in the current space from meta.
    pub(super) async fn execute_show_tags(&self) -> Result<ExecutorResult> {
        let columns = vec!["Name".to_string()];
        let space = self
            .ctx
            .space
            .as_ref()
            .ok_or_else(|| ExecutionError::InvalidOperation("No space selected".to_string()))?;

        #[cfg(feature = "distributed")]
        if let Some(client) = &self.ctx.meta_client {
            match client.get_space(space).await {
                Ok(space_info) => match client.list_tags(space_info.id).await {
                    Ok(tags) => {
                        let rows = tags
                            .into_iter()
                            .map(|t| vec![byoridb_common::Value::String(t.name)])
                            .collect();
                        return Ok(ExecutorResult {
                            columns,
                            rows,
                            latency_ms: 0,
                        });
                    }
                    Err(e) => {
                        tracing::warn!("SHOW TAGS: failed to list tags: {}", e);
                    }
                },
                Err(e) => {
                    tracing::warn!("SHOW TAGS: failed to resolve space {}: {}", space, e);
                }
            }
        }

        // Fallback: scan kvstore
        let rows = self
            .show_schema_names_from_kvstore(&SchemaKey::tag_prefix(space), ":tag:")
            .await
            .unwrap_or_default();
        Ok(ExecutorResult {
            columns,
            rows,
            latency_ms: 0,
        })
    }

    /// Execute SHOW EDGES — lists all edge types in the current space from meta.
    pub(super) async fn execute_show_edges(&self) -> Result<ExecutorResult> {
        let columns = vec!["Name".to_string()];
        let space = self
            .ctx
            .space
            .as_ref()
            .ok_or_else(|| ExecutionError::InvalidOperation("No space selected".to_string()))?;

        #[cfg(feature = "distributed")]
        if let Some(client) = &self.ctx.meta_client {
            match client.get_space(space).await {
                Ok(space_info) => match client.list_edges(space_info.id).await {
                    Ok(edges) => {
                        let rows = edges
                            .into_iter()
                            .map(|e| vec![byoridb_common::Value::String(e.name)])
                            .collect();
                        return Ok(ExecutorResult {
                            columns,
                            rows,
                            latency_ms: 0,
                        });
                    }
                    Err(e) => {
                        tracing::warn!("SHOW EDGES: failed to list edges: {}", e);
                    }
                },
                Err(e) => {
                    tracing::warn!("SHOW EDGES: failed to resolve space {}: {}", space, e);
                }
            }
        }

        // Fallback: scan kvstore
        let rows = self
            .show_schema_names_from_kvstore(&SchemaKey::edge_prefix(space), ":edge:")
            .await
            .unwrap_or_default();
        Ok(ExecutorResult {
            columns,
            rows,
            latency_ms: 0,
        })
    }

    /// Execute SHOW TAG INDEXES — lists all tag indexes in the current space.
    ///
    /// Index metadata is owned by the Meta service. Without a MetaClient the
    /// executor cannot produce a meaningful listing and returns a clear error
    /// rather than a misleading empty/placeholder result.
    pub(super) async fn execute_show_tag_indexes(&self) -> Result<ExecutorResult> {
        let columns = vec![
            "Index Name".to_string(),
            "On Tag".to_string(),
            "Fields".to_string(),
        ];
        let _space = self.require_space()?.to_string();

        // Fallback: use index_manager when meta_client is unavailable
        if !self.ctx.has_meta_client() {
            if let Some(index_manager) = &self.ctx.index_manager {
                let space_id = self.ctx.resolve_space_id().await; // resolved from space name (fixes cross-space index collapse)
                let indexes = index_manager.list_tag_indexes(space_id).await;
                let rows = indexes
                    .iter()
                    .map(|idx| {
                        vec![
                            byoridb_common::Value::String(idx.index_name.clone()),
                            byoridb_common::Value::String(idx.schema_name.clone()),
                            byoridb_common::Value::String(idx.fields.join(", ")),
                        ]
                    })
                    .collect();
                return Ok(ExecutorResult {
                    columns,
                    rows,
                    latency_ms: 0,
                });
            }
            return Ok(ExecutorResult {
                columns,
                rows: vec![],
                latency_ms: 0,
            });
        }

        #[cfg(feature = "distributed")]
        {
            let client = self.ctx.meta_client.as_ref().ok_or_else(|| {
                ExecutionError::InvalidOperation(
                    "SHOW TAG INDEXES requires a running Meta service".to_string(),
                )
            })?;

            let space_info = client
                .get_space(&_space)
                .await
                .map_err(|e| ExecutionError::InvalidOperation(format!("Meta error: {}", e)))?;

            // Build tag_id -> tag_name map for human-readable output.
            let tag_name_by_id: std::collections::HashMap<u32, String> = client
                .list_tags(space_info.id)
                .await
                .map_err(|e| ExecutionError::InvalidOperation(format!("Meta error: {}", e)))?
                .into_iter()
                .map(|t| (t.id, t.name))
                .collect();

            let indexes = client
                .list_tag_indexes(space_info.id)
                .await
                .map_err(|e| ExecutionError::InvalidOperation(format!("Meta error: {}", e)))?;

            let rows = indexes
                .into_iter()
                .map(|idx| {
                    let tag_label = tag_name_by_id
                        .get(&idx.tag_id)
                        .cloned()
                        .unwrap_or_else(|| format!("<tag_id={}>", idx.tag_id));
                    vec![
                        byoridb_common::Value::String(idx.index_name),
                        byoridb_common::Value::String(tag_label),
                        byoridb_common::Value::String(idx.fields.join(", ")),
                    ]
                })
                .collect();

            Ok(ExecutorResult {
                columns,
                rows,
                latency_ms: 0,
            })
        }
        #[cfg(not(feature = "distributed"))]
        {
            Ok(ExecutorResult {
                columns,
                rows: vec![],
                latency_ms: 0,
            })
        }
    }

    /// Execute SHOW EDGE INDEXES — lists all edge indexes in the current space.
    ///
    pub(super) async fn execute_show_edge_indexes(&self) -> Result<ExecutorResult> {
        let columns = vec![
            "Index Name".to_string(),
            "On Edge".to_string(),
            "Fields".to_string(),
        ];
        let _space = self.require_space()?.to_string();

        // Fallback: use index_manager when meta_client is unavailable
        if !self.ctx.has_meta_client() {
            if let Some(index_manager) = &self.ctx.index_manager {
                let space_id = self.ctx.resolve_space_id().await; // resolved from space name (fixes cross-space index collapse)
                let indexes = index_manager.list_edge_indexes(space_id).await;
                let rows = indexes
                    .iter()
                    .map(|idx| {
                        vec![
                            byoridb_common::Value::String(idx.index_name.clone()),
                            byoridb_common::Value::String(idx.schema_name.clone()),
                            byoridb_common::Value::String(idx.fields.join(", ")),
                        ]
                    })
                    .collect();
                return Ok(ExecutorResult {
                    columns,
                    rows,
                    latency_ms: 0,
                });
            }
            return Ok(ExecutorResult {
                columns,
                rows: vec![],
                latency_ms: 0,
            });
        }

        #[cfg(feature = "distributed")]
        {
            let client = self.ctx.meta_client.as_ref().ok_or_else(|| {
                ExecutionError::InvalidOperation(
                    "SHOW EDGE INDEXES requires a running Meta service".to_string(),
                )
            })?;

            let space_info = client
                .get_space(&_space)
                .await
                .map_err(|e| ExecutionError::InvalidOperation(format!("Meta error: {}", e)))?;

            // Build edge_id -> edge_name map for human-readable output.
            let edge_name_by_id: std::collections::HashMap<u32, String> = client
                .list_edges(space_info.id)
                .await
                .map_err(|e| ExecutionError::InvalidOperation(format!("Meta error: {}", e)))?
                .into_iter()
                .map(|e| (e.id, e.name))
                .collect();

            let indexes = client
                .list_edge_indexes(space_info.id)
                .await
                .map_err(|e| ExecutionError::InvalidOperation(format!("Meta error: {}", e)))?;

            let rows = indexes
                .into_iter()
                .map(|idx| {
                    let edge_label = edge_name_by_id
                        .get(&idx.edge_type)
                        .cloned()
                        .unwrap_or_else(|| format!("<edge_id={}>", idx.edge_type));
                    vec![
                        byoridb_common::Value::String(idx.index_name),
                        byoridb_common::Value::String(edge_label),
                        byoridb_common::Value::String(idx.fields.join(", ")),
                    ]
                })
                .collect();

            Ok(ExecutorResult {
                columns,
                rows,
                latency_ms: 0,
            })
        }
        #[cfg(not(feature = "distributed"))]
        {
            Ok(ExecutorResult {
                columns,
                rows: vec![],
                latency_ms: 0,
            })
        }
    }

    /// Scan kvstore for tag or edge schema entries.
    /// `prefix` is the kvstore prefix (e.g. `space:<name>:tag:`) and
    /// `marker` is the substring (`":tag:"` / `":edge:"`) whose suffix is the schema name.
    pub(super) async fn show_schema_names_from_kvstore(
        &self,
        prefix: &[u8],
        marker: &str,
    ) -> Result<Vec<Vec<byoridb_common::Value>>> {
        let entries = self.ctx.kvstore.scan_prefix(prefix).await?;

        let mut rows = Vec::new();
        for (k, _) in entries {
            let key_str = match std::str::from_utf8(&k) {
                Ok(s) => s,
                Err(_) => continue,
            };
            if let Some(idx) = key_str.rfind(marker) {
                let name = &key_str[idx + marker.len()..];
                // Skip index entries (which live under tag_index: / edge_index:)
                if !name.is_empty() && !name.contains(':') {
                    rows.push(vec![byoridb_common::Value::String(name.to_string())]);
                }
            }
        }
        Ok(rows)
    }

    /// Execute SHOW USERS from the persisted user records.
    ///
    /// The built-in root account is owned by the graph authentication layer and
    /// intentionally has no KV record, so it is always included explicitly.
    /// Persisted users and their roles are sorted to make the result stable.
    pub(super) async fn execute_show_users(&self) -> Result<ExecutorResult> {
        if !self.ctx.is_admin() {
            return Err(ExecutionError::InvalidOperation(
                "SHOW USERS requires GOD or ADMIN role".to_string(),
            ));
        }

        let entries = self
            .ctx
            .kvstore
            .scan_prefix(USER_KEY_PREFIX.as_bytes())
            .await?;
        let mut users = vec![("root".to_string(), vec!["GOD".to_string()])];

        for (key, value) in entries {
            let key = std::str::from_utf8(&key).map_err(|_| {
                ExecutionError::InvalidOperation(
                    "Stored user metadata contains a non-UTF-8 key".to_string(),
                )
            })?;
            let username = key.strip_prefix(USER_KEY_PREFIX).ok_or_else(|| {
                ExecutionError::InvalidOperation(format!(
                    "Stored user metadata has an invalid key: {key}"
                ))
            })?;
            if username.is_empty() {
                return Err(ExecutionError::InvalidOperation(
                    "Stored user metadata has an empty username".to_string(),
                ));
            }

            // Legacy versions could persist a root record. The graph layer's
            // built-in root account remains authoritative, so never duplicate
            // or override its GOD role in this listing.
            if username.eq_ignore_ascii_case("root") {
                continue;
            }

            let metadata: serde_json::Value = serde_json::from_slice(&value)?;
            let stored_username = metadata
                .get("username")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    ExecutionError::InvalidOperation(format!(
                        "Stored user metadata for {username} is missing username"
                    ))
                })?;
            if stored_username != username {
                return Err(ExecutionError::InvalidOperation(format!(
                    "Stored user metadata key {username} does not match username {stored_username}"
                )));
            }

            let role_values = metadata
                .get("roles")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| {
                    ExecutionError::InvalidOperation(format!(
                        "Stored user metadata for {username} has no roles array"
                    ))
                })?;
            let mut roles = role_values
                .iter()
                .map(|role| {
                    role.as_str().map(ToString::to_string).ok_or_else(|| {
                        ExecutionError::InvalidOperation(format!(
                            "Stored user metadata for {username} contains a non-string role"
                        ))
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            roles.sort();
            roles.dedup();
            users.push((username.to_string(), roles));
        }

        users.sort_by(|a, b| a.0.cmp(&b.0));
        let rows = users
            .into_iter()
            .map(|(username, roles)| {
                vec![
                    byoridb_common::Value::String(username),
                    byoridb_common::Value::String(roles.join(", ")),
                ]
            })
            .collect();

        Ok(ExecutorResult {
            columns: vec!["Name".to_string(), "Role".to_string()],
            rows,
            latency_ms: 0,
        })
    }

    /// Format a [`byoridb_meta::schema::VidType`] for display.
    #[cfg(feature = "distributed")]
    pub(super) fn format_vid_type(vid_type: &byoridb_meta::schema::VidType) -> String {
        match vid_type {
            byoridb_meta::schema::VidType::Int64 => "INT64".to_string(),
            byoridb_meta::schema::VidType::FixedString(len) => format!("FIXED_STRING({})", len),
        }
    }

    /// Format a [`byoridb_meta::schema::DataType`] for display.
    #[cfg(feature = "distributed")]
    pub(super) fn format_data_type(dt: &byoridb_meta::schema::DataType) -> String {
        use byoridb_meta::schema::DataType::*;
        match dt {
            Bool => "BOOL".to_string(),
            Int8 => "INT8".to_string(),
            Int16 => "INT16".to_string(),
            Int32 => "INT32".to_string(),
            Int64 => "INT64".to_string(),
            Float => "FLOAT".to_string(),
            Double => "DOUBLE".to_string(),
            String => "STRING".to_string(),
            FixedString(len) => format!("FIXED_STRING({})", len),
            Timestamp => "TIMESTAMP".to_string(),
            Date => "DATE".to_string(),
            Time => "TIME".to_string(),
            DateTime => "DATETIME".to_string(),
            Geography => "GEOGRAPHY".to_string(),
        }
    }

    /// Execute DESCRIBE TAG|EDGE|SPACE <name>.
    pub(super) async fn execute_describe(
        &self,
        plan: crate::plan::DescribePlan,
    ) -> Result<ExecutorResult> {
        match plan {
            crate::plan::DescribePlan::Tag(name) => self.execute_describe_tag(&name).await,
            crate::plan::DescribePlan::Edge(name) => self.execute_describe_edge(&name).await,
            crate::plan::DescribePlan::Space(name) => self.execute_describe_space(&name).await,
            crate::plan::DescribePlan::TagIndex(name) => {
                self.execute_describe_index(&name, true).await
            }
            crate::plan::DescribePlan::EdgeIndex(name) => {
                self.execute_describe_index(&name, false).await
            }
            crate::plan::DescribePlan::Class(name) => self.execute_describe_class(&name).await,
        }
    }

    /// Describe a tag: returns Field, Type, Null, Default rows.
    pub(super) async fn execute_describe_tag(&self, name: &str) -> Result<ExecutorResult> {
        let space = self
            .ctx
            .space
            .as_ref()
            .ok_or_else(|| ExecutionError::InvalidOperation("No space selected".to_string()))?;

        // MetaClient path
        #[cfg(feature = "distributed")]
        if let Some(client) = &self.ctx.meta_client {
            let space_info = client.get_space(space).await.map_err(|e| {
                ExecutionError::InvalidOperation(format!(
                    "Failed to resolve space {}: {}",
                    space, e
                ))
            })?;
            let tag = client.get_tag(space_info.id, name).await.map_err(|e| {
                ExecutionError::InvalidOperation(format!("Tag {} not found: {}", name, e))
            })?;
            return Ok(Self::schema_fields_to_result(&tag.fields));
        }

        // Standalone fallback: read schema JSON from kvstore
        Self::describe_schema_from_kvstore(&self.ctx, &SchemaKey::tag(space, name), name).await
    }

    /// Describe an edge: returns Field, Type, Null, Default rows.
    pub(super) async fn execute_describe_edge(&self, name: &str) -> Result<ExecutorResult> {
        let space = self
            .ctx
            .space
            .as_ref()
            .ok_or_else(|| ExecutionError::InvalidOperation("No space selected".to_string()))?;

        // MetaClient path
        #[cfg(feature = "distributed")]
        if let Some(client) = &self.ctx.meta_client {
            let space_info = client.get_space(space).await.map_err(|e| {
                ExecutionError::InvalidOperation(format!(
                    "Failed to resolve space {}: {}",
                    space, e
                ))
            })?;
            let edge = client.get_edge(space_info.id, name).await.map_err(|e| {
                ExecutionError::InvalidOperation(format!("Edge {} not found: {}", name, e))
            })?;
            return Ok(Self::schema_fields_to_result(&edge.fields));
        }

        // Standalone fallback: read schema JSON from kvstore
        Self::describe_schema_from_kvstore(&self.ctx, &SchemaKey::edge(space, name), name).await
    }

    /// Read a tag or edge schema from kvstore and render as DESCRIBE output.
    /// Schema JSON format: {"name": "...", "properties": [{"name": "f", "data_type": "String", "nullable": true}, ...]}
    async fn describe_schema_from_kvstore(
        ctx: &crate::context::ExecutionContext,
        key: &[u8],
        schema_name: &str,
    ) -> Result<ExecutorResult> {
        let bytes = ctx
            .kvstore
            .get(key)
            .await?
            .ok_or_else(|| ExecutionError::TagNotFound(schema_name.to_string()))?;

        let schema: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|e| ExecutionError::InvalidOperation(format!("Corrupt schema: {}", e)))?;

        let columns = vec![
            "Field".to_string(),
            "Type".to_string(),
            "Null".to_string(),
            "Default".to_string(),
        ];

        let rows = schema["properties"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|p| {
                        let field_name = p["name"].as_str()?.to_string();
                        let data_type = p
                            .get("data_type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("STRING")
                            .to_uppercase();
                        let nullable = p.get("nullable").and_then(|v| v.as_bool()).unwrap_or(true);
                        let default_val = p
                            .get("default_value")
                            .filter(|v| !v.is_null())
                            .map(|v| byoridb_common::Value::String(v.to_string()))
                            .unwrap_or(byoridb_common::Value::Null(byoridb_common::NullType::Null));
                        Some(vec![
                            byoridb_common::Value::String(field_name),
                            byoridb_common::Value::String(data_type),
                            byoridb_common::Value::String(
                                if nullable { "YES" } else { "NO" }.to_string(),
                            ),
                            default_val,
                        ])
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(ExecutorResult {
            columns,
            rows,
            latency_ms: 0,
        })
    }

    /// Describe a space: returns Property/Value rows for space metadata.
    pub(super) async fn execute_describe_space(&self, name: &str) -> Result<ExecutorResult> {
        #[cfg(feature = "distributed")]
        {
            let client = self.ctx.meta_client.as_ref().ok_or_else(|| {
                ExecutionError::InvalidOperation(
                    "Meta client not configured; cannot describe space".to_string(),
                )
            })?;

            let space = client.get_space(name).await.map_err(|e| {
                ExecutionError::InvalidOperation(format!("Space {} not found: {}", name, e))
            })?;

            let partition_strategy = format!("{:?}", space.partition_strategy);

            let rows = vec![
                vec![
                    byoridb_common::Value::String("ID".to_string()),
                    byoridb_common::Value::String(space.id.to_string()),
                ],
                vec![
                    byoridb_common::Value::String("Name".to_string()),
                    byoridb_common::Value::String(space.name),
                ],
                vec![
                    byoridb_common::Value::String("Partition Num".to_string()),
                    byoridb_common::Value::String(space.partition_num.to_string()),
                ],
                vec![
                    byoridb_common::Value::String("Replica Factor".to_string()),
                    byoridb_common::Value::String(space.replica_factor.to_string()),
                ],
                vec![
                    byoridb_common::Value::String("Vid Type".to_string()),
                    byoridb_common::Value::String(Self::format_vid_type(&space.vid_type)),
                ],
                vec![
                    byoridb_common::Value::String("Partition Strategy".to_string()),
                    byoridb_common::Value::String(partition_strategy),
                ],
            ];

            Ok(ExecutorResult {
                columns: vec!["Property".to_string(), "Value".to_string()],
                rows,
                latency_ms: 0,
            })
        }
        #[cfg(not(feature = "distributed"))]
        {
            let key = SchemaKey::space(name);
            let data = self.ctx.kvstore.get(&key).await?.ok_or_else(|| {
                ExecutionError::InvalidOperation(format!("Space {} not found", name))
            })?;
            let space: serde_json::Value = serde_json::from_slice(&data)
                .map_err(|e| ExecutionError::InvalidOperation(format!("Corrupt space: {}", e)))?;
            let rows = vec![
                vec![
                    byoridb_common::Value::String("ID".to_string()),
                    byoridb_common::Value::String(
                        space
                            .get("id")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0)
                            .to_string(),
                    ),
                ],
                vec![
                    byoridb_common::Value::String("Name".to_string()),
                    byoridb_common::Value::String(name.to_string()),
                ],
                vec![
                    byoridb_common::Value::String("Partition Num".to_string()),
                    byoridb_common::Value::String(
                        space
                            .get("partition_num")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0)
                            .to_string(),
                    ),
                ],
                vec![
                    byoridb_common::Value::String("Replica Factor".to_string()),
                    byoridb_common::Value::String(
                        space
                            .get("replica_factor")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0)
                            .to_string(),
                    ),
                ],
            ];
            Ok(ExecutorResult {
                columns: vec!["Property".to_string(), "Value".to_string()],
                rows,
                latency_ms: 0,
            })
        }
    }

    /// Convert schema fields into a tabular result (Field, Type, Null, Default).
    #[cfg(feature = "distributed")]
    pub(super) fn schema_fields_to_result(
        fields: &[byoridb_meta::schema::Field],
    ) -> ExecutorResult {
        let rows = fields
            .iter()
            .map(|f| {
                vec![
                    byoridb_common::Value::String(f.name.clone()),
                    byoridb_common::Value::String(Self::format_data_type(&f.data_type)),
                    byoridb_common::Value::String(
                        if f.nullable { "YES" } else { "NO" }.to_string(),
                    ),
                    match &f.default {
                        Some(d) => byoridb_common::Value::String(d.clone()),
                        None => byoridb_common::Value::Null(byoridb_common::NullType::Null),
                    },
                ]
            })
            .collect();

        ExecutorResult {
            columns: vec![
                "Field".to_string(),
                "Type".to_string(),
                "Null".to_string(),
                "Default".to_string(),
            ],
            rows,
            latency_ms: 0,
        }
    }

    /// DESCRIBE TAG INDEX / DESCRIBE EDGE INDEX — show index field details.
    pub(super) async fn execute_describe_index(
        &self,
        index_name: &str,
        is_tag: bool,
    ) -> Result<ExecutorResult> {
        let _space = self
            .ctx
            .space
            .as_ref()
            .ok_or_else(|| ExecutionError::InvalidOperation("No space selected".to_string()))?;
        let space_id = self.ctx.resolve_space_id().await; // resolved from space name (fixes cross-space index collapse)
        let index_manager = self.ctx.index_manager.as_ref().ok_or_else(|| {
            ExecutionError::InvalidOperation("No index manager available".to_string())
        })?;

        let indexes = if is_tag {
            index_manager.list_tag_indexes(space_id).await
        } else {
            index_manager.list_edge_indexes(space_id).await
        };

        let idx = indexes
            .iter()
            .find(|i| i.index_name.eq_ignore_ascii_case(index_name))
            .ok_or_else(|| {
                ExecutionError::IndexNotFound(format!("Index '{}' not found", index_name))
            })?;

        let rows = idx
            .fields
            .iter()
            .map(|field| {
                vec![
                    byoridb_common::Value::String(field.clone()),
                    byoridb_common::Value::String("STRING".to_string()),
                ]
            })
            .collect();

        Ok(ExecutorResult {
            columns: vec!["Field".to_string(), "Type".to_string()],
            rows,
            latency_ms: 0,
        })
    }

    /// Execute SHOW PARTS - display partition allocation
    pub(super) async fn execute_show_parts(&self) -> Result<ExecutorResult> {
        let space = self
            .ctx
            .space
            .as_ref()
            .ok_or_else(|| ExecutionError::InvalidOperation("No space selected".to_string()))?;

        // Try to get partition allocation from meta client
        #[cfg(feature = "distributed")]
        if let Some(client) = &self.ctx.meta_client {
            match client.get_space(space).await {
                Ok(space_info) => {
                    match client.get_parts_alloc(space_info.id).await {
                        Ok(allocs) => {
                            let mut rows = Vec::new();
                            for alloc in allocs {
                                let hosts_str = alloc
                                    .hosts
                                    .iter()
                                    .map(|(h, p)| format!("{}:{}", h, p))
                                    .collect::<Vec<_>>()
                                    .join(", ");
                                // Leader is first host in the list (simplified)
                                let leader_str = alloc
                                    .hosts
                                    .first()
                                    .map(|(h, p)| format!("{}:{}", h, p))
                                    .unwrap_or_else(|| "N/A".to_string());
                                rows.push(vec![
                                    byoridb_common::Value::Int(alloc.part_id as i64),
                                    byoridb_common::Value::String(leader_str),
                                    byoridb_common::Value::String(hosts_str),
                                ]);
                            }
                            return Ok(ExecutorResult {
                                columns: vec![
                                    "Part ID".to_string(),
                                    "Leader".to_string(),
                                    "Hosts".to_string(),
                                ],
                                rows,
                                latency_ms: 0,
                            });
                        }
                        Err(e) => {
                            tracing::warn!("Failed to get parts allocation: {}", e);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to get space info: {}", e);
                }
            }
        }

        // Fallback: meta client missing or unavailable. Returning empty is
        // more honest than a hardcoded placeholder row; a warning is logged so
        // operators can tell the difference from "genuinely no partitions".
        tracing::warn!(
            "SHOW PARTS returning empty result for space {}: meta client unavailable or lookup failed",
            space
        );
        Ok(ExecutorResult {
            columns: vec![
                "Part ID".to_string(),
                "Leader".to_string(),
                "Hosts".to_string(),
            ],
            rows: vec![],
            latency_ms: 0,
        })
    }

    /// Execute SHOW HOSTS - display storage hosts
    pub(super) async fn execute_show_hosts(&self) -> Result<ExecutorResult> {
        let columns = vec![
            "Host".to_string(),
            "Port".to_string(),
            "Status".to_string(),
            "Leader Count".to_string(),
            "Part Count".to_string(),
        ];

        #[cfg(feature = "distributed")]
        let rows = {
            let hosts = self.fetch_hosts_or_warn().await.unwrap_or_default();
            hosts.into_iter().map(Self::host_info_to_row).collect()
        };
        #[cfg(not(feature = "distributed"))]
        let rows = {
            tracing::warn!("SHOW HOSTS returning empty result: distributed feature is disabled");
            Vec::new()
        };

        Ok(ExecutorResult {
            columns,
            rows,
            latency_ms: 0,
        })
    }

    /// Fetch the host list from the meta service, logging a warning and
    /// returning `None` when the meta client is missing or the RPC fails.
    /// This keeps the hardcoded-free behaviour of `SHOW HOSTS` simple: an
    /// empty table is rendered instead of a misleading placeholder row.
    #[cfg(feature = "distributed")]
    pub(super) async fn fetch_hosts_or_warn(&self) -> Option<Vec<byoridb_meta::HostInfo>> {
        let Some(client) = self.ctx.meta_client.as_ref() else {
            tracing::warn!(
                "SHOW HOSTS returning empty result: meta client not configured in this context"
            );
            return None;
        };
        client.list_hosts().await.map_or_else(
            |e| {
                tracing::warn!("SHOW HOSTS meta lookup failed: {}", e);
                None
            },
            Some,
        )
    }

    // ===== SHOW STATS =====

    /// SHOW STATS — count vertices and edges per type in the current space.
    pub(super) async fn execute_show_stats(&self) -> Result<ExecutorResult> {
        let space = match self.ctx.space.as_ref() {
            Some(s) => s.clone(),
            None => {
                return Ok(ExecutorResult {
                    columns: vec!["Error".to_string()],
                    rows: vec![vec![byoridb_common::Value::String(
                        "No space selected".to_string(),
                    )]],
                    latency_ms: 0,
                })
            }
        };

        let mut rows: Vec<Vec<byoridb_common::Value>> = Vec::new();
        let mut total_vertices = 0i64;
        let mut total_edges = 0i64;

        // Count vertices by first tag name
        let vertex_prefix = format!("{}:vertex:", space);
        let vertex_entries = self
            .ctx
            .kvstore
            .scan_prefix(vertex_prefix.as_bytes())
            .await?;
        // Seed every schema-defined tag with 0 so an empty tag reports 0 rather
        // than being absent from the output — validation tools can then tell
        // "0 rows" apart from "tag does not exist".
        let mut tag_counts: std::collections::BTreeMap<String, i64> = self
            .show_schema_names_from_kvstore(&SchemaKey::tag_prefix(&space), ":tag:")
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(|r| match r.into_iter().next() {
                Some(byoridb_common::Value::String(s)) => Some((s, 0)),
                _ => None,
            })
            .collect();
        for (_, value) in &vertex_entries {
            total_vertices += 1;
            if let Ok(v) = byoridb_codec::VertexCodec::decode_vertex(value) {
                let tag_name = v
                    .tags
                    .first()
                    .map(|t| t.name.clone())
                    .unwrap_or_else(|| "_unknown".to_string());
                *tag_counts.entry(tag_name).or_insert(0) += 1;
            }
        }
        for (tag, count) in &tag_counts {
            rows.push(vec![
                byoridb_common::Value::String("Tag".to_string()),
                byoridb_common::Value::String(tag.clone()),
                byoridb_common::Value::Int(*count),
            ]);
        }

        // Count edges by type (segment 3 in key)
        let edge_prefix = format!("{}:edge:", space);
        let edge_entries = self.ctx.kvstore.scan_prefix(edge_prefix.as_bytes()).await?;
        // Seed every schema-defined edge type with 0 (same rationale as tags).
        let mut edge_counts: std::collections::BTreeMap<String, i64> = self
            .show_schema_names_from_kvstore(&SchemaKey::edge_prefix(&space), ":edge:")
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(|r| match r.into_iter().next() {
                Some(byoridb_common::Value::String(s)) => Some((s, 0)),
                _ => None,
            })
            .collect();
        for (key, _) in &edge_entries {
            total_edges += 1;
            if let Ok(key_s) = std::str::from_utf8(key) {
                let edge_type = key_s.split(':').nth(3).unwrap_or("_unknown");
                *edge_counts.entry(edge_type.to_string()).or_insert(0) += 1;
            }
        }
        for (edge_type, count) in &edge_counts {
            rows.push(vec![
                byoridb_common::Value::String("Edge".to_string()),
                byoridb_common::Value::String(edge_type.clone()),
                byoridb_common::Value::Int(*count),
            ]);
        }

        // Sort: tags first, edges second, each alphabetically
        rows.sort_by(|a, b| {
            let ta = a[0].to_string();
            let tb = b[0].to_string();
            ta.cmp(&tb)
                .then_with(|| a[1].to_string().cmp(&b[1].to_string()))
        });

        // Append totals row
        rows.push(vec![
            byoridb_common::Value::String("Total".to_string()),
            byoridb_common::Value::String("vertices".to_string()),
            byoridb_common::Value::Int(total_vertices),
        ]);
        rows.push(vec![
            byoridb_common::Value::String("Total".to_string()),
            byoridb_common::Value::String("edges".to_string()),
            byoridb_common::Value::Int(total_edges),
        ]);

        Ok(ExecutorResult {
            columns: vec!["Type".to_string(), "Name".to_string(), "Count".to_string()],
            rows,
            latency_ms: 0,
        })
    }

    // ===== SHOW SESSIONS =====

    pub(super) async fn execute_show_sessions(&self) -> Result<ExecutorResult> {
        if !self.ctx.is_admin() {
            return Err(ExecutionError::InvalidOperation(
                "SHOW SESSIONS requires GOD or ADMIN role".to_string(),
            ));
        }

        // The Graph service intercepts SHOW SESSIONS and supplies its live
        // SessionManager. Reaching this executor-only path means no truthful
        // session source exists, so fail instead of returning a fake empty set.
        Err(ExecutionError::InvalidOperation(
            "SHOW SESSIONS requires the Graph service session manager and is unavailable in an executor-only context"
                .to_string(),
        ))
    }

    // ===== SHOW CREATE TAG / SHOW CREATE EDGE =====

    pub(super) async fn execute_show_create_tag(&self, tag_name: &str) -> Result<ExecutorResult> {
        let space = self
            .ctx
            .space
            .as_ref()
            .ok_or_else(|| ExecutionError::InvalidOperation("No space selected".to_string()))?;
        let schema_key = crate::key::SchemaKey::tag(space, tag_name);
        let ddl = if let Some(data) = self.ctx.kvstore.get(&schema_key).await? {
            schema_json_to_create_ddl(tag_name, "TAG", &data)
        } else {
            format!("CREATE TAG {} ()", tag_name)
        };
        Ok(ExecutorResult {
            columns: vec!["Tag".to_string(), "Create Tag".to_string()],
            rows: vec![vec![
                byoridb_common::Value::String(tag_name.to_string()),
                byoridb_common::Value::String(ddl),
            ]],
            latency_ms: 0,
        })
    }

    pub(super) async fn execute_show_create_edge(&self, edge_name: &str) -> Result<ExecutorResult> {
        let space = self
            .ctx
            .space
            .as_ref()
            .ok_or_else(|| ExecutionError::InvalidOperation("No space selected".to_string()))?;
        let schema_key = crate::key::SchemaKey::edge(space, edge_name);
        let ddl = if let Some(data) = self.ctx.kvstore.get(&schema_key).await? {
            schema_json_to_create_ddl(edge_name, "EDGE", &data)
        } else {
            format!("CREATE EDGE {} ()", edge_name)
        };
        Ok(ExecutorResult {
            columns: vec!["Edge".to_string(), "Create Edge".to_string()],
            rows: vec![vec![
                byoridb_common::Value::String(edge_name.to_string()),
                byoridb_common::Value::String(ddl),
            ]],
            latency_ms: 0,
        })
    }

    // ===== SHOW TAG/EDGE INDEX STATUS =====

    pub(super) async fn execute_show_index_statuses(&self, is_tag: bool) -> Result<ExecutorResult> {
        let _space = self
            .ctx
            .space
            .as_ref()
            .ok_or_else(|| ExecutionError::InvalidOperation("No space selected".to_string()))?;
        let space_id = self.ctx.resolve_space_id().await; // resolved from space name (fixes cross-space index collapse)
        let index_manager = match self.ctx.index_manager.as_ref() {
            Some(m) => m,
            None => {
                return Ok(ExecutorResult {
                    columns: vec![
                        "Name".to_string(),
                        "Tag/Edge".to_string(),
                        "Fields".to_string(),
                        "Status".to_string(),
                    ],
                    rows: vec![],
                    latency_ms: 0,
                })
            }
        };

        let indexes = if is_tag {
            index_manager.list_tag_indexes(space_id).await
        } else {
            index_manager.list_edge_indexes(space_id).await
        };

        let rows = indexes
            .iter()
            .map(|idx| {
                vec![
                    byoridb_common::Value::String(idx.index_name.clone()),
                    byoridb_common::Value::String(idx.schema_name.clone()),
                    byoridb_common::Value::String(idx.fields.join(", ")),
                    // All indexes are considered DONE (no background rebuild tracking)
                    byoridb_common::Value::String("DONE".to_string()),
                ]
            })
            .collect();

        Ok(ExecutorResult {
            columns: vec![
                "Name".to_string(),
                "Tag/Edge".to_string(),
                "Fields".to_string(),
                "Status".to_string(),
            ],
            rows,
            latency_ms: 0,
        })
    }

    /// Convert a [`byoridb_meta::HostInfo`] into a row of values matching the
    /// `SHOW HOSTS` column order.
    #[cfg(feature = "distributed")]
    pub(super) fn host_info_to_row(h: byoridb_meta::HostInfo) -> Vec<byoridb_common::Value> {
        let status = match h.status {
            byoridb_meta::HostLiveness::Online => "ONLINE",
            byoridb_meta::HostLiveness::Offline => "OFFLINE",
        };
        vec![
            byoridb_common::Value::String(h.host),
            byoridb_common::Value::Int(h.port as i64),
            byoridb_common::Value::String(status.to_string()),
            byoridb_common::Value::Int(h.leader_count as i64),
            byoridb_common::Value::Int(h.part_count as i64),
        ]
    }
}

/// Build a CREATE DDL string from the raw JSON bytes stored in KVStore.
/// The stored format uses `"properties"` array (see `describe_schema_from_kvstore`).
fn schema_json_to_create_ddl(name: &str, kind: &str, data: &[u8]) -> String {
    let schema: serde_json::Value = match serde_json::from_slice(data) {
        Ok(v) => v,
        Err(_) => return format!("CREATE {} {} ()", kind, name),
    };

    let fields: Vec<String> = schema["properties"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|p| {
                    let field_name = p["name"].as_str()?.to_string();
                    let data_type = p
                        .get("data_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("STRING")
                        .to_uppercase();
                    let nullable = p.get("nullable").and_then(|v| v.as_bool()).unwrap_or(true);
                    let default_str = p
                        .get("default_value")
                        .filter(|v| !v.is_null())
                        .map(|v| format!(" DEFAULT {}", v.to_string().trim_matches('"')))
                        .unwrap_or_default();
                    let not_null = if nullable { "" } else { " NOT NULL" };
                    Some(format!(
                        "{} {}{}{}",
                        field_name, data_type, not_null, default_str
                    ))
                })
                .collect()
        })
        .unwrap_or_default();

    format!("CREATE {} {} ({})", kind, name, fields.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ExecutionContext;
    use byoridb_kvstore::store::MemoryKVStore;
    use byoridb_kvstore::KVStore as _;
    use std::sync::Arc;

    #[tokio::test]
    async fn show_users_lists_root_and_persisted_users_in_stable_order() {
        let kvstore = Arc::new(MemoryKVStore::new());
        for (username, roles) in [("zoe", vec!["USER", "ADMIN", "USER"]), ("alice", vec![])] {
            let metadata = serde_json::json!({
                "username": username,
                "password_hash": "test-only",
                "roles": roles,
                "enabled": true
            });
            kvstore
                .put(
                    format!("{USER_KEY_PREFIX}{username}").as_bytes(),
                    &serde_json::to_vec(&metadata).unwrap(),
                )
                .await
                .unwrap();
        }

        // A legacy root record must neither duplicate nor override built-in root.
        kvstore
            .put(
                format!("{USER_KEY_PREFIX}RoOt").as_bytes(),
                &serde_json::to_vec(&serde_json::json!({
                    "username": "RoOt",
                    "password_hash": "legacy",
                    "roles": ["GUEST"],
                    "enabled": true
                }))
                .unwrap(),
            )
            .await
            .unwrap();

        let executor = Executor::new(Arc::new(
            ExecutionContext::new(kvstore).with_caller_roles(vec!["GOD".to_string()]),
        ));
        let result = executor.execute_show_users().await.unwrap();

        assert_eq!(result.columns, vec!["Name", "Role"]);
        assert_eq!(
            result.rows,
            vec![
                vec![
                    byoridb_common::Value::String("alice".to_string()),
                    byoridb_common::Value::String(String::new()),
                ],
                vec![
                    byoridb_common::Value::String("root".to_string()),
                    byoridb_common::Value::String("GOD".to_string()),
                ],
                vec![
                    byoridb_common::Value::String("zoe".to_string()),
                    byoridb_common::Value::String("ADMIN, USER".to_string()),
                ],
            ]
        );
    }

    #[tokio::test]
    async fn show_sessions_errors_without_graph_session_manager() {
        let executor = Executor::new(Arc::new(
            ExecutionContext::new(Arc::new(MemoryKVStore::new()))
                .with_caller_roles(vec!["ADMIN".to_string()]),
        ));

        let err = executor.execute_show_sessions().await.unwrap_err();
        match err {
            ExecutionError::InvalidOperation(message) => {
                assert!(message.contains("Graph service session manager"));
                assert!(message.contains("executor-only"));
            }
            other => panic!("expected InvalidOperation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn user_and_session_listings_require_admin_role() {
        let executor = Executor::new(Arc::new(
            ExecutionContext::new(Arc::new(MemoryKVStore::new()))
                .with_caller_roles(vec!["USER".to_string()]),
        ));

        for err in [
            executor.execute_show_users().await.unwrap_err(),
            executor.execute_show_sessions().await.unwrap_err(),
        ] {
            match err {
                ExecutionError::InvalidOperation(message) => {
                    assert!(message.contains("GOD or ADMIN"), "message was: {message}");
                }
                other => panic!("expected InvalidOperation, got {other:?}"),
            }
        }
    }
}

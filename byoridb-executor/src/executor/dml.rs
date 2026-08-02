// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use super::{Executor, ExecutorResult};
use crate::error::{ExecutionError, Result};
use crate::key::SchemaKey;
use byoridb_codec::{
    EdgeData as CodecEdgeData, TagData as CodecTagData, VertexCodec, VertexData as CodecVertexData,
};

impl Executor {
    /// Execute INSERT statement
    pub(super) async fn execute_insert(
        &self,
        plan: crate::plan::InsertPlan,
    ) -> Result<ExecutorResult> {
        match plan {
            crate::plan::InsertPlan::Vertex { space, vertices } => {
                // Use plan.space or fallback to ctx.space
                let effective_space = if space.is_empty() {
                    self.ctx
                        .space
                        .as_ref()
                        .ok_or_else(|| {
                            ExecutionError::InvalidOperation("No space selected".to_string())
                        })?
                        .clone()
                } else {
                    space
                };
                let vid_type = crate::vid::space_vid_type(&self.ctx, &effective_space).await?;
                // Fetch the tag-index list once (not per row), then collect every
                // KV write into a single batch so the whole multi-row INSERT
                // commits in one redb transaction (one fsync) instead of one per
                // put. This is the dominant cost for bulk loads. The batch is
                // also atomic — a multi-row INSERT now applies all-or-nothing.
                let space_id = self.ctx.resolve_space_id().await;
                let tag_indexes = match self.ctx.index_manager.as_ref() {
                    Some(im) => im.list_tag_indexes(space_id).await,
                    None => Vec::new(),
                };
                let mut batch: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
                // T-트랙: (현재뷰 키, blob) 쌍을 모아 커밋 후 이력에 append.
                let mut vertex_versions: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
                let mut inserted = 0i64;
                // Properties whose dense vectors changed → their persisted HNSW
                // index (R-2b) is now stale and must be rebuilt on next query.
                let mut dirty_vec_props: std::collections::HashSet<String> =
                    std::collections::HashSet::new();
                for vertex in vertices {
                    // Schema validation: verify each tag and its fields exist
                    for tag in &vertex.tags {
                        self.validate_tag_props(&effective_space, &tag.name, &tag.props)
                            .await?;
                    }
                    // Write-time shape validation (SHACL-style constraints):
                    // reject a vertex violating any in-scope shape. The flattened
                    // property map mirrors the RECOMMEND filter convention (bare +
                    // `{tag}.{prop}`). No-op when no shapes are declared.
                    {
                        let mut tag_names = Vec::with_capacity(vertex.tags.len());
                        let mut sprops: std::collections::HashMap<String, byoridb_common::Value> =
                            std::collections::HashMap::new();
                        for tag in &vertex.tags {
                            tag_names.push(tag.name.clone());
                            for (k, v) in &tag.props {
                                sprops.insert(format!("{}.{}", tag.name, k), v.clone());
                                sprops.insert(k.clone(), v.clone());
                            }
                        }
                        self.validate_write_shapes(&effective_space, &tag_names, &sprops)
                            .await?;
                    }
                    // Materialize a string mapping only after schema/shape
                    // validation succeeds, so rejected writes do not leave
                    // behind unused VID metadata.
                    let internal_vid = crate::vid::resolve_vid(
                        &self.ctx,
                        &effective_space,
                        vid_type,
                        &vertex.vid,
                        true,
                    )
                    .await?
                    .expect("creating a string VID mapping always resolves");
                    let key = format!("{}:vertex:{}", effective_space, internal_vid);
                    // Convert plan TagData to codec TagData and use Proto encoding
                    let codec_vertex = CodecVertexData {
                        vid: internal_vid,
                        tags: vertex
                            .tags
                            .iter()
                            .map(|t| CodecTagData {
                                name: t.name.clone(),
                                properties: t.props.clone(),
                            })
                            .collect(),
                    };
                    let data = VertexCodec::encode_vertex(&codec_vertex)
                        .map_err(|e| ExecutionError::Io(std::io::Error::other(e.to_string())))?;
                    let key_bytes = key.into_bytes();
                    // T-트랙: 현재뷰 blob을 이력 버전으로도 기록.
                    vertex_versions.push((key_bytes.clone(), data.clone()));
                    batch.push((key_bytes, data));
                    // Tag-vid secondary index for label-only MATCH acceleration.
                    // Key: {space}:tagvid:{tag_name}:{vid} → empty value.
                    for tag in &codec_vertex.tags {
                        let tagvid_key =
                            format!("{}:tagvid:{}:{}", effective_space, tag.name, vertex.vid);
                        batch.push((tagvid_key.into_bytes(), Vec::new()));
                    }
                    // Dense embedding side-store (PLAN.md R-2a): any numeric-list
                    // property is mirrored as packed f32 under {space}:vec:{prop}:{vid}
                    // so cosine KNN scans packed floats instead of decoding vertices.
                    for tag in &codec_vertex.tags {
                        for (prop, value) in &tag.properties {
                            if let Some(bytes) = crate::executor::recommend::pack_embedding(value) {
                                let vkey = crate::key::SchemaKey::vec_data(
                                    &effective_space,
                                    prop,
                                    internal_vid,
                                );
                                batch.push((vkey, bytes));
                                dirty_vec_props.insert(prop.clone());
                            }
                        }
                    }
                    for index in &tag_indexes {
                        for tag in codec_vertex
                            .tags
                            .iter()
                            .filter(|tag| tag.name == index.schema_name)
                        {
                            let values = index
                                .fields
                                .iter()
                                .map(|field| {
                                    self.byoridb_value_to_index_value(
                                        tag.properties
                                            .get(field)
                                            .unwrap_or(&byoridb_common::Value::null()),
                                    )
                                })
                                .collect::<Vec<_>>();
                            let idx_key = byoridb_storage::KeyUtils::tag_index_key(
                                1,
                                index.id,
                                &values,
                                internal_vid,
                            );
                            batch.push((idx_key, Vec::new()));
                        }
                    }
                    inserted += 1;
                }
                // T-트랙 v1.1: 현재뷰 쓰기 + 이력 버전 append 를 단일 트랜잭션으로
                // 커밋 (dual-write 원자성).
                self.ctx
                    .kvstore
                    .batch_apply(batch, Vec::new(), Self::build_versions(vertex_versions))
                    .await?;
                // Invalidate persisted vector indexes (R-2b) for embedding props
                // touched by this INSERT — rebuilt lazily on next BY EMBEDDING query.
                for prop in &dirty_vec_props {
                    self.mark_vector_index_dirty(&effective_space, prop).await?;
                }
                Ok(ExecutorResult {
                    columns: vec!["Inserted".to_string()],
                    rows: vec![vec![byoridb_common::Value::Int(inserted)]],
                    latency_ms: 0,
                })
            }
            crate::plan::InsertPlan::Edge { space, edges } => {
                // Use plan.space or fallback to ctx.space
                let effective_space = if space.is_empty() {
                    self.ctx
                        .space
                        .as_ref()
                        .ok_or_else(|| {
                            ExecutionError::InvalidOperation("No space selected".to_string())
                        })?
                        .clone()
                } else {
                    space
                };
                let vid_type = crate::vid::space_vid_type(&self.ctx, &effective_space).await?;
                // Collect all KV writes into one batch → single redb commit
                // (one fsync) per multi-row INSERT, applied atomically.
                let space_id = self.ctx.resolve_space_id().await;
                let edge_indexes = match self.ctx.index_manager.as_ref() {
                    Some(im) => im.list_edge_indexes(space_id).await,
                    None => Vec::new(),
                };
                let mut batch: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
                // T-트랙: (현재뷰 엣지 키, blob) 쌍을 모아 커밋 후 이력에 append.
                let mut edge_versions: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
                let mut inserted = 0i64;
                // Asserted triples to feed ontology materialization (O-5) after commit.
                let mut new_triples: Vec<(i64, String, i64)> = Vec::new();
                // Edge-degree counters (precomputed COUNT): accumulate per
                // (etype, vid) so multi-row INSERT sums in one increment.
                let mut deg_in: std::collections::HashMap<(String, i64), i64> =
                    std::collections::HashMap::new();
                let mut deg_out: std::collections::HashMap<(String, i64), i64> =
                    std::collections::HashMap::new();
                // Current-view edge keys newly asserted in this batch, to count a
                // duplicate edge's degree at most once.
                let mut seen_edges: std::collections::HashSet<Vec<u8>> =
                    std::collections::HashSet::new();
                for edge in edges {
                    // Schema validation precedes mapping creation so an invalid
                    // edge row cannot materialize otherwise-unused VID records.
                    let edge_type_name = edge.edge_type.clone();
                    self.validate_edge_props(&effective_space, &edge_type_name, &edge.props)
                        .await?;
                    let src = crate::vid::resolve_vid(
                        &self.ctx,
                        &effective_space,
                        vid_type,
                        &edge.src,
                        true,
                    )
                    .await?
                    .expect("creating a string VID mapping always resolves");
                    let dst = crate::vid::resolve_vid(
                        &self.ctx,
                        &effective_space,
                        vid_type,
                        &edge.dst,
                        true,
                    )
                    .await?
                    .expect("creating a string VID mapping always resolves");
                    let key = format!(
                        "{}:edge:{}:{}:{}:{}",
                        effective_space, src, edge_type_name, dst, edge.ranking
                    );
                    // Use Proto encoding for edge data
                    let codec_edge = CodecEdgeData {
                        src_vid: src,
                        dst_vid: dst,
                        edge_type: edge_type_name.clone(),
                        ranking: edge.ranking,
                        properties: edge.props.clone(),
                    };
                    let data = VertexCodec::encode_edge(&codec_edge)
                        .map_err(|e| ExecutionError::Io(std::io::Error::other(e.to_string())))?;
                    let key_bytes = key.into_bytes();
                    // A duplicate edge (same src/type/dst/rank) overwrites the one
                    // current-view key, so its degree must be counted at most once
                    // — both within this batch (`seen_edges`) and against edges
                    // already persisted. Otherwise re-inserting inflated the degree
                    // counter, and a later delete left a ghost count.
                    let edge_is_new = seen_edges.insert(key_bytes.clone())
                        && self.ctx.kvstore.get(&key_bytes).await?.is_none();
                    // T-트랙: 현재뷰 엣지 blob을 이력 버전으로도 기록.
                    edge_versions.push((key_bytes.clone(), data.clone()));
                    batch.push((key_bytes, data.clone()));
                    // Reverse-edge index: {space}:in-edge:{dst}:{edge_type}:{src}:{ranking}
                    // holds the same denormalized payload so reverse traversal
                    // is an O(in-degree) prefix scan (see algo::get_incoming_neighbors).
                    let in_edge_key = SchemaKey::in_edge_data(
                        &effective_space,
                        dst,
                        &edge_type_name,
                        src,
                        edge.ranking,
                    );
                    batch.push((in_edge_key, data));
                    for index in edge_indexes
                        .iter()
                        .filter(|index| index.schema_name == edge_type_name)
                    {
                        let values = index
                            .fields
                            .iter()
                            .map(|field| {
                                self.byoridb_value_to_index_value(
                                    codec_edge
                                        .properties
                                        .get(field)
                                        .unwrap_or(&byoridb_common::Value::null()),
                                )
                            })
                            .collect::<Vec<_>>();
                        let idx_key = byoridb_storage::KeyUtils::edge_index_key(
                            1,
                            index.id,
                            &values,
                            codec_edge.src_vid,
                            codec_edge.ranking,
                            codec_edge.dst_vid,
                        );
                        batch.push((idx_key, Vec::new()));
                    }
                    new_triples.push((src, edge_type_name.clone(), dst));
                    if edge_is_new {
                        *deg_in.entry((edge_type_name.clone(), dst)).or_insert(0) += 1;
                        *deg_out.entry((edge_type_name.clone(), src)).or_insert(0) += 1;
                    }
                    inserted += 1;
                }
                // T-트랙 v1.1: 현재뷰 쓰기 + 이력 버전 append 단일 트랜잭션.
                self.ctx
                    .kvstore
                    .batch_apply(batch, Vec::new(), Self::build_versions(edge_versions))
                    .await?;
                // Maintain edge-degree counters for the asserted edges (used by
                // the COUNT fast-path). Atomic increment so concurrent inserts
                // don't lose updates.
                let counter_deltas: Vec<(Vec<u8>, i64)> = deg_in
                    .iter()
                    .map(|((et, dst), d)| {
                        (SchemaKey::indeg_counter(&effective_space, et, *dst), *d)
                    })
                    .chain(deg_out.iter().map(|((et, src), d)| {
                        (SchemaKey::outdeg_counter(&effective_space, et, *src), *d)
                    }))
                    .collect();
                if !counter_deltas.is_empty() {
                    self.ctx.kvstore.add_counters(counter_deltas).await?;
                }
                // owl:sameAs node-equivalence merge (O-8): collapse equivalence
                // classes onto a canonical representative *before* O-5 so
                // forward-chaining runs over the canonicalized graph (D10).
                self.merge_sameas_triples(&effective_space, &new_triples)
                    .await?;
                // Ontology forward-chaining materialization (O-5): derive and
                // persist entailed edges. No-op if the space declares no
                // semantic relations. Runs after the asserted edges are
                // committed so the closure reads them. sameAs triples are
                // handled by the merge above, not by O-5 rules — exclude them.
                let onto_triples: Vec<_> = new_triples
                    .into_iter()
                    .filter(|(_, p, _)| p != crate::executor::sameas::SAMEAS_EDGE)
                    .collect();
                self.materialize_inserted_edges(&effective_space, onto_triples)
                    .await?;
                Ok(ExecutorResult {
                    columns: vec!["Inserted".to_string()],
                    rows: vec![vec![byoridb_common::Value::Int(inserted)]],
                    latency_ms: 0,
                })
            }
        }
    }

    /// Validate that a tag exists in the space and that all provided property
    /// names are declared in the tag schema.
    /// Whether a value is assignable to a declared column type (serde variant
    /// name of `ast::DataType`, e.g. "Int64"/"String"). `None` (a non-unit type
    /// such as `FixedString(n)`) is treated leniently. Null is allowed here
    /// (nullability is a separate concern); Int coerces to float/temporal types;
    /// composite values (lists/maps, e.g. embeddings) are not strictly typed.
    fn value_assignable(value: &byoridb_common::Value, type_name: Option<&str>) -> bool {
        use byoridb_common::Value as V;
        let Some(t) = type_name else { return true };
        match value {
            V::Null(_) => true,
            V::Bool(_) => t == "Bool",
            V::Int(_) => matches!(
                t,
                "Int8"
                    | "Int16"
                    | "Int32"
                    | "Int64"
                    | "Float"
                    | "Double"
                    | "Timestamp"
                    | "Date"
                    | "Time"
                    | "DateTime"
            ),
            V::Float(_) => matches!(t, "Float" | "Double"),
            V::String(_) => matches!(
                t,
                "String" | "Geography" | "Date" | "Time" | "DateTime" | "Timestamp"
            ),
            _ => true,
        }
    }

    pub(super) async fn validate_tag_props(
        &self,
        space: &str,
        tag_name: &str,
        props: &std::collections::HashMap<String, byoridb_common::Value>,
    ) -> Result<()> {
        let tag_key = SchemaKey::tag(space, tag_name);
        let schema_bytes = self
            .ctx
            .kvstore
            .get(&tag_key)
            .await?
            .ok_or_else(|| ExecutionError::TagNotFound(tag_name.to_string()))?;

        let schema: serde_json::Value = serde_json::from_slice(&schema_bytes)
            .map_err(|e| ExecutionError::InvalidOperation(format!("Corrupt tag schema: {}", e)))?;

        let empty = vec![];
        let props_arr = schema["properties"].as_array().unwrap_or(&empty);
        for (field, value) in props {
            let Some(pdef) = props_arr
                .iter()
                .find(|p| p["name"].as_str() == Some(field.as_str()))
            else {
                return Err(ExecutionError::InvalidOperation(format!(
                    "Tag '{}' has no field '{}'",
                    tag_name, field
                )));
            };
            if !Self::value_assignable(value, pdef["data_type"].as_str()) {
                return Err(ExecutionError::TypeMismatch(format!(
                    "field '{}' of tag '{}' expects type {}, got {:?}",
                    field, tag_name, pdef["data_type"], value
                )));
            }
        }
        Ok(())
    }

    /// Validate that an edge type exists in the space and that all provided
    /// property names are declared in the edge schema.
    pub(super) async fn validate_edge_props(
        &self,
        space: &str,
        edge_type: &str,
        props: &std::collections::HashMap<String, byoridb_common::Value>,
    ) -> Result<()> {
        let edge_key = SchemaKey::edge(space, edge_type);
        let schema_bytes = self
            .ctx
            .kvstore
            .get(&edge_key)
            .await?
            .ok_or_else(|| ExecutionError::EdgeNotFound(edge_type.to_string()))?;

        let schema: serde_json::Value = serde_json::from_slice(&schema_bytes)
            .map_err(|e| ExecutionError::InvalidOperation(format!("Corrupt edge schema: {}", e)))?;

        let empty = vec![];
        let props_arr = schema["properties"].as_array().unwrap_or(&empty);
        for (field, value) in props {
            let Some(pdef) = props_arr
                .iter()
                .find(|p| p["name"].as_str() == Some(field.as_str()))
            else {
                return Err(ExecutionError::InvalidOperation(format!(
                    "Edge type '{}' has no field '{}'",
                    edge_type, field
                )));
            };
            if !Self::value_assignable(value, pdef["data_type"].as_str()) {
                return Err(ExecutionError::TypeMismatch(format!(
                    "field '{}' of edge '{}' expects type {}, got {:?}",
                    field, edge_type, pdef["data_type"], value
                )));
            }
        }
        Ok(())
    }

    /// Execute UPDATE statement
    pub(super) async fn execute_update(
        &self,
        plan: crate::plan::UpdatePlan,
    ) -> Result<ExecutorResult> {
        // Use plan.space or fallback to ctx.space (same as INSERT)
        let effective_space = if plan.space.is_empty() {
            self.ctx
                .space
                .as_ref()
                .ok_or_else(|| ExecutionError::InvalidOperation("No space selected".to_string()))?
                .clone()
        } else {
            plan.space.clone()
        };

        let vid_type = crate::vid::space_vid_type(&self.ctx, &effective_space).await?;
        let vid = crate::vid::resolve_vid(&self.ctx, &effective_space, vid_type, &plan.vid, true)
            .await?
            .expect("UPDATE upsert always resolves a VID");
        let tag_name = plan.tag_name.as_ref().ok_or_else(|| {
            ExecutionError::InvalidOperation("Tag name required for UPDATE".to_string())
        })?;

        // Schema validation: verify tag and updated fields exist
        self.validate_tag_props(&effective_space, tag_name, &plan.updates)
            .await?;

        // Key format matches INSERT: {space}:vertex:{vid}
        let key = format!("{}:vertex:{}", effective_space, vid);

        let existing_data = self.ctx.kvstore.get(key.as_bytes()).await?;
        let existed = existing_data.is_some();

        // Upsert: create vertex if it does not exist yet
        let mut vertex_data = if let Some(data) = existing_data {
            VertexCodec::decode_vertex(&data)
                .map_err(|e| ExecutionError::Io(std::io::Error::other(e.to_string())))?
        } else {
            byoridb_codec::VertexData {
                vid,
                tags: vec![byoridb_codec::TagData {
                    name: tag_name.clone(),
                    properties: std::collections::HashMap::new(),
                }],
            }
        };

        // Snapshot pre-update indexed values so stale tag-index entries can be
        // removed after the write (UPDATE previously left secondary indexes
        // pointing at the old property values).
        let old_tag_props: std::collections::HashMap<
            String,
            std::collections::HashMap<String, byoridb_common::Value>,
        > = vertex_data
            .tags
            .iter()
            .map(|t| (t.name.clone(), t.properties.clone()))
            .collect();

        // Evaluate the optional WHEN condition against the CURRENT vertex state.
        // A conditional UPDATE whose condition is false — or one targeting a
        // vertex that does not exist — is a no-op. Previously the condition was
        // ignored and the write always applied.
        if let Some(cond) = &plan.conditions {
            let pass = existed && {
                let mut current: std::collections::HashMap<String, byoridb_common::Value> =
                    std::collections::HashMap::new();
                for tag in &vertex_data.tags {
                    for (k, v) in &tag.properties {
                        current.insert(format!("{}.{}", tag.name, k), v.clone());
                        current.entry(k.clone()).or_insert_with(|| v.clone());
                    }
                }
                let ectx = crate::evaluator::EvalContext::new().with_current(current);
                crate::evaluator::Evaluator::evaluate_condition(cond, &ectx)?
            };
            if !pass {
                return Ok(ExecutorResult {
                    columns: vec!["Updated".to_string()],
                    rows: vec![vec![byoridb_common::Value::Int(0)]],
                    latency_ms: 0,
                });
            }
        }

        // Update props in the matching tag; add the tag if absent
        let tag_exists = vertex_data.tags.iter().any(|t| t.name == *tag_name);
        if !tag_exists {
            vertex_data.tags.push(byoridb_codec::TagData {
                name: tag_name.clone(),
                properties: std::collections::HashMap::new(),
            });
        }
        for tag in &mut vertex_data.tags {
            if tag.name == *tag_name {
                for (k, v) in &plan.updates {
                    tag.properties.insert(k.clone(), v.clone());
                }
            }
        }

        // Write-time shape validation over the *post-update* vertex state, so a
        // partial UPDATE that would leave the vertex non-conformant is rejected
        // before it is persisted. No-op when no shapes are declared.
        {
            let mut tag_names = Vec::with_capacity(vertex_data.tags.len());
            let mut sprops: std::collections::HashMap<String, byoridb_common::Value> =
                std::collections::HashMap::new();
            for tag in &vertex_data.tags {
                tag_names.push(tag.name.clone());
                for (k, v) in &tag.properties {
                    sprops.insert(format!("{}.{}", tag.name, k), v.clone());
                    sprops.insert(k.clone(), v.clone());
                }
            }
            self.validate_write_shapes(&effective_space, &tag_names, &sprops)
                .await?;
        }

        // Re-encode using Proto format
        let encoded_data = VertexCodec::encode_vertex(&vertex_data)
            .map_err(|e| ExecutionError::Io(std::io::Error::other(e.to_string())))?;

        // T-트랙 v1.1: 현재뷰 갱신 + 이력 버전 append 단일 트랜잭션.
        let key_bytes = key.into_bytes();
        self.ctx
            .kvstore
            .batch_apply(
                vec![(key_bytes.clone(), encoded_data.clone())],
                Vec::new(),
                Self::build_versions(vec![(key_bytes, encoded_data)]),
            )
            .await?;

        // Maintain tag secondary indexes: remove entries for the OLD property
        // values and add entries for the NEW ones. Without this, LOOKUP kept
        // returning the pre-update value and missed the new one.
        if let Some(im) = self.ctx.index_manager.as_ref() {
            let space_id = self.ctx.resolve_space_id().await;
            for index in im.list_tag_indexes(space_id).await {
                if let Some(old_props) = old_tag_props.get(&index.schema_name) {
                    let old_values: Vec<_> = index
                        .fields
                        .iter()
                        .map(|f| {
                            self.byoridb_value_to_index_value(
                                old_props.get(f).unwrap_or(&byoridb_common::Value::null()),
                            )
                        })
                        .collect();
                    im.delete_tag_index(1, index.id, &old_values, vid)
                        .await
                        .map_err(|e| {
                            ExecutionError::InvalidOperation(format!(
                                "index update (delete old) failed: {e}"
                            ))
                        })?;
                }
                if let Some(tag) = vertex_data
                    .tags
                    .iter()
                    .find(|t| t.name == index.schema_name)
                {
                    let new_values: Vec<_> = index
                        .fields
                        .iter()
                        .map(|f| {
                            self.byoridb_value_to_index_value(
                                tag.properties
                                    .get(f)
                                    .unwrap_or(&byoridb_common::Value::null()),
                            )
                        })
                        .collect();
                    im.insert_tag_index(1, index.id, &new_values, vid)
                        .await
                        .map_err(|e| {
                            ExecutionError::InvalidOperation(format!(
                                "index update (insert new) failed: {e}"
                            ))
                        })?;
                }
            }
        }

        // Keep the dense embedding side-store consistent (PLAN.md R-2a): an
        // updated numeric-list property is re-mirrored; a property that is no
        // longer a numeric list has its stale dense entry removed. Without this,
        // embedding KNN would silently score the old vector (the read-path
        // existence check can't catch it — the vertex is still live).
        for (k, v) in &plan.updates {
            let vkey = crate::key::SchemaKey::vec_data(&effective_space, k, vid);
            match crate::executor::recommend::pack_embedding(v) {
                Some(bytes) => {
                    self.ctx.kvstore.put(&vkey, &bytes).await?;
                    // Persisted HNSW index (R-2b) is now stale for this property.
                    self.mark_vector_index_dirty(&effective_space, k).await?;
                }
                None => self.ctx.kvstore.delete(&vkey).await?,
            }
        }

        Ok(ExecutorResult {
            columns: vec!["Updated".to_string()],
            rows: vec![vec![byoridb_common::Value::Int(1)]],
            latency_ms: 0,
        })
    }

    /// Execute DELETE statement
    pub(super) async fn execute_delete(
        &self,
        plan: crate::plan::DeletePlan,
    ) -> Result<ExecutorResult> {
        // Use plan.space or fallback to ctx.space
        let effective_space = if plan.space.is_empty() {
            self.ctx
                .space
                .as_ref()
                .ok_or_else(|| ExecutionError::InvalidOperation("No space selected".to_string()))?
                .clone()
        } else {
            plan.space.clone()
        };

        let vid_type = crate::vid::space_vid_type(&self.ctx, &effective_space).await?;
        let mut resolved_vids = Vec::with_capacity(plan.vids.len());
        for vid in &plan.vids {
            if let Some(internal) =
                crate::vid::resolve_vid(&self.ctx, &effective_space, vid_type, vid, false).await?
            {
                resolved_vids.push(internal);
            }
        }

        // O-8 D7: owl:sameAs merges are irreversible (insertion-only), so refuse
        // to delete a vertex entangled in one — either a non-representative
        // member (its facts moved elsewhere) or a representative that absorbed
        // others. Allowing it would orphan or lose the merged class.
        for vid in &resolved_vids {
            let rep = crate::ontology::representative_of(&self.ctx, &effective_space, *vid).await?;
            if rep != *vid {
                return Err(ExecutionError::InvalidOperation(format!(
                    "vertex {} was merged into representative {} via owl:sameAs; \
                     deletion is unsupported (insertion-only)",
                    vid, rep
                )));
            }
            if !crate::ontology::members_of(&self.ctx, &effective_space, *vid)
                .await?
                .is_empty()
            {
                return Err(ExecutionError::InvalidOperation(format!(
                    "vertex {} is a sameAs representative with merged members; \
                     deletion is unsupported (insertion-only)",
                    vid
                )));
            }
        }

        // Build all keys at once
        let keys: Vec<Vec<u8>> = resolved_vids
            .iter()
            .map(|vid| format!("{}:vertex:{}", effective_space, vid).into_bytes())
            .collect();

        // Batch check existence
        let results = self.ctx.kvstore.batch_get(&keys).await?;

        // Collect keys that exist for deletion. For each deleted vertex, also
        // remove its dense embedding entries and invalidate the affected vector
        // indexes (R-2a/R-2b) — otherwise the deleted vector lingers in the
        // dense store and is re-indexed on the next HNSW rebuild, degrading recall.
        let mut deleted = 0;
        let mut dirty_vec_props: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        // T-트랙: 삭제된 정점의 현재뷰 키를 tombstone 버전으로 기록.
        let mut tombstones: Vec<Vec<u8>> = Vec::new();
        // 현재뷰에서 지울 키들 — 마지막에 tombstone append 와 한 트랜잭션으로.
        let mut del_keys: Vec<Vec<u8>> = Vec::new();
        // Tag indexes to keep consistent as vertices are removed (DELETE
        // previously left stale index entries pointing at deleted vids).
        let del_tag_indexes = match self.ctx.index_manager.as_ref() {
            Some(im) => im.list_tag_indexes(self.ctx.resolve_space_id().await).await,
            None => Vec::new(),
        };
        for (vid, (key, exists)) in resolved_vids.iter().zip(keys.iter().zip(results.iter())) {
            let Some(data) = exists else { continue };
            let vertex = VertexCodec::decode_vertex(data).ok();

            // Evaluate the optional WHERE condition; skip vertices that don't
            // match. Previously the condition was ignored and every listed vid
            // was deleted unconditionally (`DELETE VERTEX 1 WHERE false` deleted).
            if let Some(cond) = &plan.conditions {
                let pass = match &vertex {
                    Some(v) => {
                        let mut current: std::collections::HashMap<String, byoridb_common::Value> =
                            std::collections::HashMap::new();
                        for tag in &v.tags {
                            for (k, val) in &tag.properties {
                                current.insert(format!("{}.{}", tag.name, k), val.clone());
                                current.entry(k.clone()).or_insert_with(|| val.clone());
                            }
                        }
                        let ectx = crate::evaluator::EvalContext::new().with_current(current);
                        crate::evaluator::Evaluator::evaluate_condition(cond, &ectx)?
                    }
                    None => false,
                };
                if !pass {
                    continue;
                }
            }

            del_keys.push(key.clone());
            tombstones.push(key.clone());
            deleted += 1;
            if let Some(vertex) = &vertex {
                for tag in &vertex.tags {
                    for (prop, value) in &tag.properties {
                        if crate::executor::recommend::pack_embedding(value).is_some() {
                            let vkey =
                                crate::key::SchemaKey::vec_data(&effective_space, prop, *vid);
                            del_keys.push(vkey);
                            dirty_vec_props.insert(prop.clone());
                        }
                    }
                }
                // Remove tag secondary-index entries for the deleted vertex.
                if let Some(im) = self.ctx.index_manager.as_ref() {
                    for index in &del_tag_indexes {
                        if let Some(tag) = vertex.tags.iter().find(|t| t.name == index.schema_name)
                        {
                            let values: Vec<_> = index
                                .fields
                                .iter()
                                .map(|f| {
                                    self.byoridb_value_to_index_value(
                                        tag.properties
                                            .get(f)
                                            .unwrap_or(&byoridb_common::Value::null()),
                                    )
                                })
                                .collect();
                            im.delete_tag_index(1, index.id, &values, *vid)
                                .await
                                .map_err(|e| {
                                    ExecutionError::InvalidOperation(format!(
                                        "index delete failed: {e}"
                                    ))
                                })?;
                        }
                    }
                }
            }
        }
        // T-트랙 v1.1: 현재뷰 삭제 + tombstone append 단일 트랜잭션.
        self.ctx
            .kvstore
            .batch_apply(Vec::new(), del_keys, Self::build_tombstones(tombstones))
            .await?;
        for prop in &dirty_vec_props {
            self.mark_vector_index_dirty(&effective_space, prop).await?;
        }

        // O-9 retraction: re-materialize so any inferred vertex types / edges
        // tied to the removed vertices are retracted. No-op without semantics.
        self.rematerialize_space(&effective_space).await?;

        Ok(ExecutorResult {
            columns: vec!["Deleted".to_string()],
            rows: vec![vec![byoridb_common::Value::Int(deleted)]],
            latency_ms: 0,
        })
    }

    pub(super) async fn execute_delete_edge(
        &self,
        plan: crate::plan::DeleteEdgePlan,
    ) -> Result<ExecutorResult> {
        let effective_space = if plan.space.is_empty() {
            self.ctx
                .space
                .as_ref()
                .ok_or_else(|| ExecutionError::InvalidOperation("No space selected".to_string()))?
                .clone()
        } else {
            plan.space.clone()
        };
        let vid_type = crate::vid::space_vid_type(&self.ctx, &effective_space).await?;

        // O-8 D7: a sameAs edge is what triggered an irreversible merge — deleting
        // it cannot un-merge the equivalence class, so reject rather than mislead.
        if plan.edge_name == crate::executor::sameas::SAMEAS_EDGE {
            return Err(ExecutionError::InvalidOperation(
                "deleting a sameAs edge is unsupported — owl:sameAs merges are \
                 irreversible (insertion-only)"
                    .to_string(),
            ));
        }

        let mut deleted = 0i64;
        // Asserted edges actually removed — seed set for incremental retraction.
        let mut deleted_edges: Vec<(i64, String, i64)> = Vec::new();
        // Edge-degree counter decrements for edges that actually existed.
        let mut deg_in: std::collections::HashMap<(String, i64), i64> =
            std::collections::HashMap::new();
        let mut deg_out: std::collections::HashMap<(String, i64), i64> =
            std::collections::HashMap::new();
        // T-트랙: 삭제된 엣지의 현재뷰 키를 tombstone 버전으로 기록.
        let mut tombstones: Vec<Vec<u8>> = Vec::new();
        // 현재뷰에서 지울 키들(정방향 + 역방향 인덱스) — tombstone 과 한 트랜잭션.
        let mut del_keys: Vec<Vec<u8>> = Vec::new();
        // 삭제가 트랜잭션 끝으로 지연되므로, 같은 edge ref 가 중복 나열되면 두 번
        // 세지 않도록 배치 내 중복을 걸러낸다 (기존 즉시-삭제 시절의 동작 유지).
        let mut seen_refs: std::collections::HashSet<Vec<u8>> = std::collections::HashSet::new();
        for (external_src, external_dst, ranking) in &plan.edge_refs {
            let Some(src) =
                crate::vid::resolve_vid(&self.ctx, &effective_space, vid_type, external_src, false)
                    .await?
            else {
                continue;
            };
            let Some(dst) =
                crate::vid::resolve_vid(&self.ctx, &effective_space, vid_type, external_dst, false)
                    .await?
            else {
                continue;
            };
            // Key format: {space}:edge:{src}:{edge_type}:{dst}:{ranking}
            let key = format!(
                "{}:edge:{}:{}:{}:{}",
                effective_space, src, plan.edge_name, dst, ranking
            );
            if seen_refs.insert(key.as_bytes().to_vec())
                && self.ctx.kvstore.get(key.as_bytes()).await?.is_some()
            {
                del_keys.push(key.as_bytes().to_vec());
                tombstones.push(key.as_bytes().to_vec());
                // Keep the reverse-edge index in sync (written by INSERT EDGE).
                let in_edge_key =
                    SchemaKey::in_edge_data(&effective_space, dst, &plan.edge_name, src, *ranking);
                del_keys.push(in_edge_key);
                *deg_in.entry((plan.edge_name.clone(), dst)).or_insert(0) -= 1;
                *deg_out.entry((plan.edge_name.clone(), src)).or_insert(0) -= 1;
                deleted += 1;
                deleted_edges.push((src, plan.edge_name.clone(), dst));
            }
        }
        // T-트랙 v1.1: 현재뷰 삭제 + tombstone append 단일 트랜잭션.
        self.ctx
            .kvstore
            .batch_apply(Vec::new(), del_keys, Self::build_tombstones(tombstones))
            .await?;

        // Apply the degree-counter decrements (atomic; ≤0 removes the key).
        let counter_deltas: Vec<(Vec<u8>, i64)> =
            deg_in
                .iter()
                .map(|((et, dst), d)| (SchemaKey::indeg_counter(&effective_space, et, *dst), *d))
                .chain(deg_out.iter().map(|((et, src), d)| {
                    (SchemaKey::outdeg_counter(&effective_space, et, *src), *d)
                }))
                .collect();
        if !counter_deltas.is_empty() {
            self.ctx.kvstore.add_counters(counter_deltas).await?;
        }

        // O-10 Phase 3 retraction: incremental DRed over provenance — overdelete
        // the deleted edges' dependent closure, then rederive from surviving
        // neighbors. Touches only the affected region (vs O-9 full re-mat).
        // No-op for spaces without semantic relations.
        self.retract_edges_incremental(&effective_space, deleted_edges)
            .await?;

        Ok(ExecutorResult {
            columns: vec!["Deleted".to_string()],
            rows: vec![vec![byoridb_common::Value::Int(deleted)]],
            latency_ms: 0,
        })
    }
}

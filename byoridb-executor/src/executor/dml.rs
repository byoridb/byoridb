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
                // Fetch the tag-index list once (not per row), then collect every
                // KV write into a single batch so the whole multi-row INSERT
                // commits in one redb transaction (one fsync) instead of one per
                // put. This is the dominant cost for bulk loads. The batch is
                // also atomic — a multi-row INSERT now applies all-or-nothing.
                let space_id = self.ctx.space_id.unwrap_or(1);
                let tag_indexes = match self.ctx.index_manager.as_ref() {
                    Some(im) => im.list_tag_indexes(space_id).await,
                    None => Vec::new(),
                };
                let mut batch: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
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
                    let key = format!("{}:vertex:{}", effective_space, vertex.vid);
                    // Convert plan TagData to codec TagData and use Proto encoding
                    let codec_vertex = CodecVertexData {
                        vid: vertex.vid,
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
                    batch.push((key.into_bytes(), data));
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
                                    vertex.vid,
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
                                1, index.id, &values, vertex.vid,
                            );
                            batch.push((idx_key, Vec::new()));
                        }
                    }
                    inserted += 1;
                }
                if !batch.is_empty() {
                    self.ctx.kvstore.batch_put(batch).await?;
                }
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
                // Collect all KV writes into one batch → single redb commit
                // (one fsync) per multi-row INSERT, applied atomically.
                let space_id = self.ctx.space_id.unwrap_or(1);
                let edge_indexes = match self.ctx.index_manager.as_ref() {
                    Some(im) => im.list_edge_indexes(space_id).await,
                    None => Vec::new(),
                };
                let mut batch: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
                let mut inserted = 0i64;
                // Asserted triples to feed ontology materialization (O-5) after commit.
                let mut new_triples: Vec<(i64, String, i64)> = Vec::new();
                for edge in edges {
                    // Schema validation: verify edge type and its fields exist
                    let edge_type_name = edge.edge_type.clone();
                    self.validate_edge_props(&effective_space, &edge_type_name, &edge.props)
                        .await?;
                    let key = format!(
                        "{}:edge:{}:{}:{}:{}",
                        effective_space, edge.src, edge_type_name, edge.dst, edge.ranking
                    );
                    // Use Proto encoding for edge data
                    let codec_edge = CodecEdgeData {
                        src_vid: edge.src,
                        dst_vid: edge.dst,
                        edge_type: edge_type_name.clone(),
                        ranking: edge.ranking,
                        properties: edge.props.clone(),
                    };
                    let data = VertexCodec::encode_edge(&codec_edge)
                        .map_err(|e| ExecutionError::Io(std::io::Error::other(e.to_string())))?;
                    batch.push((key.into_bytes(), data.clone()));
                    // Reverse-edge index: {space}:in-edge:{dst}:{edge_type}:{src}:{ranking}
                    // holds the same denormalized payload so reverse traversal
                    // is an O(in-degree) prefix scan (see algo::get_incoming_neighbors).
                    let in_edge_key = SchemaKey::in_edge_data(
                        &effective_space,
                        edge.dst,
                        &edge_type_name,
                        edge.src,
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
                    new_triples.push((edge.src, edge_type_name.clone(), edge.dst));
                    inserted += 1;
                }
                if !batch.is_empty() {
                    self.ctx.kvstore.batch_put(batch).await?;
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

        let defined: std::collections::HashSet<String> = schema["properties"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|p| p["name"].as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        for field in props.keys() {
            if !defined.contains(field) {
                return Err(ExecutionError::InvalidOperation(format!(
                    "Tag '{}' has no field '{}'",
                    tag_name, field
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

        let defined: std::collections::HashSet<String> = schema["properties"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|p| p["name"].as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        for field in props.keys() {
            if !defined.contains(field) {
                return Err(ExecutionError::InvalidOperation(format!(
                    "Edge type '{}' has no field '{}'",
                    edge_type, field
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

        let vid = plan.vid;
        let tag_name = plan.tag_name.as_ref().ok_or_else(|| {
            ExecutionError::InvalidOperation("Tag name required for UPDATE".to_string())
        })?;

        // Schema validation: verify tag and updated fields exist
        self.validate_tag_props(&effective_space, tag_name, &plan.updates)
            .await?;

        // Key format matches INSERT: {space}:vertex:{vid}
        let key = format!("{}:vertex:{}", effective_space, vid);

        let existing_data = self.ctx.kvstore.get(key.as_bytes()).await?;

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

        // Re-encode using Proto format
        let encoded_data = VertexCodec::encode_vertex(&vertex_data)
            .map_err(|e| ExecutionError::Io(std::io::Error::other(e.to_string())))?;

        self.ctx.kvstore.put(key.as_bytes(), &encoded_data).await?;

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

        // O-8 D7: owl:sameAs merges are irreversible (insertion-only), so refuse
        // to delete a vertex entangled in one — either a non-representative
        // member (its facts moved elsewhere) or a representative that absorbed
        // others. Allowing it would orphan or lose the merged class.
        for vid in &plan.vids {
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
        let keys: Vec<Vec<u8>> = plan
            .vids
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
        for (vid, (key, exists)) in plan.vids.iter().zip(keys.iter().zip(results.iter())) {
            let Some(data) = exists else { continue };
            self.ctx.kvstore.delete(key).await?;
            deleted += 1;
            if let Ok(vertex) = VertexCodec::decode_vertex(data) {
                for tag in &vertex.tags {
                    for (prop, value) in &tag.properties {
                        if crate::executor::recommend::pack_embedding(value).is_some() {
                            let vkey =
                                crate::key::SchemaKey::vec_data(&effective_space, prop, *vid);
                            self.ctx.kvstore.delete(&vkey).await?;
                            dirty_vec_props.insert(prop.clone());
                        }
                    }
                }
            }
        }
        for prop in &dirty_vec_props {
            self.mark_vector_index_dirty(&effective_space, prop).await?;
        }

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
        for (src, dst, ranking) in &plan.edge_refs {
            // Key format: {space}:edge:{src}:{edge_type}:{dst}:{ranking}
            let key = format!(
                "{}:edge:{}:{}:{}:{}",
                effective_space, src, plan.edge_name, dst, ranking
            );
            if self.ctx.kvstore.get(key.as_bytes()).await?.is_some() {
                self.ctx.kvstore.delete(key.as_bytes()).await?;
                // Keep the reverse-edge index in sync (written by INSERT EDGE).
                let in_edge_key = SchemaKey::in_edge_data(
                    &effective_space,
                    *dst,
                    &plan.edge_name,
                    *src,
                    *ranking,
                );
                self.ctx.kvstore.delete(&in_edge_key).await?;
                deleted += 1;
            }
        }

        Ok(ExecutorResult {
            columns: vec!["Deleted".to_string()],
            rows: vec![vec![byoridb_common::Value::Int(deleted)]],
            latency_ms: 0,
        })
    }
}

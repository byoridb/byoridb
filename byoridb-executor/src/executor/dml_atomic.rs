//! A statement-local write overlay. DML helpers may perform many logical KV
//! operations, but none becomes visible until their complete write set and
//! asserted history are committed by one Memory/redb `batch_apply`.
//! The caller holds the real store's per-space write lock for planning + commit.
use super::{temporal::VersionRow, Executor, ExecutorResult};
use crate::error::{ExecutionError, Result};
use crate::plan::ExecutionPlan;
use async_trait::async_trait;
use byoridb_kvstore::{KVStore, KVStoreError, Result as KvResult, VersionRecord};
use byoridb_storage::{IndexDef, IndexManager, IndexType};
use parking_lot::Mutex;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Default)]
struct Changes {
    values: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
    versions: Vec<VersionRow>,
    bytes: usize,
}
struct MutationStore {
    base: Arc<dyn KVStore>,
    changes: Mutex<Changes>,
    max_bytes: usize,
}
impl MutationStore {
    fn new(base: Arc<dyn KVStore>, max_memory_mb: usize) -> Self {
        Self {
            base,
            changes: Mutex::new(Changes::default()),
            max_bytes: max_memory_mb.saturating_mul(1024 * 1024),
        }
    }
    fn set(&self, key: Vec<u8>, value: Option<Vec<u8>>) -> KvResult<()> {
        let mut changes = self.changes.lock();
        let old = changes
            .values
            .get(&key)
            .map_or(0, |v| key.len() + v.as_ref().map_or(0, Vec::len) + 64);
        let new = key.len() + value.as_ref().map_or(0, Vec::len) + 64;
        let total = changes.bytes.saturating_sub(old).saturating_add(new);
        self.check_size(total)?;
        changes.bytes = total;
        changes.values.insert(key, value);
        Ok(())
    }
    fn check_size(&self, bytes: usize) -> KvResult<()> {
        if self.max_bytes != 0 && bytes > self.max_bytes {
            return Err(KVStoreError::Io(std::io::Error::other(
                "DML staging exceeds max_memory_mb; no statement writes were committed",
            )));
        }
        Ok(())
    }
    async fn commit(&self) -> KvResult<()> {
        let changes = std::mem::take(&mut *self.changes.lock());
        let mut puts = Vec::new();
        let mut deletes = Vec::new();
        for (key, value) in changes.values {
            match value {
                Some(value) => puts.push((key, value)),
                None => deletes.push(key),
            }
        }
        if puts.is_empty() && deletes.is_empty() && changes.versions.is_empty() {
            return Ok(());
        }
        self.base.batch_apply(puts, deletes, changes.versions).await
    }
}
#[async_trait]
impl KVStore for MutationStore {
    async fn get(&self, key: &[u8]) -> KvResult<Option<Vec<u8>>> {
        if let Some(value) = self.changes.lock().values.get(key) {
            return Ok(value.clone());
        }
        self.base.get(key).await
    }
    async fn put(&self, key: &[u8], value: &[u8]) -> KvResult<()> {
        self.set(key.to_vec(), Some(value.to_vec()))
    }
    async fn put_if_absent(&self, key: &[u8], value: &[u8]) -> KvResult<Option<Vec<u8>>> {
        if let Some(existing) = self.get(key).await? {
            return Ok(Some(existing));
        }
        self.put(key, value).await?;
        Ok(None)
    }
    async fn delete(&self, key: &[u8]) -> KvResult<()> {
        self.set(key.to_vec(), None)
    }
    async fn batch_put(&self, pairs: Vec<(Vec<u8>, Vec<u8>)>) -> KvResult<()> {
        for (key, value) in pairs {
            self.set(key, Some(value))?;
        }
        Ok(())
    }
    async fn batch_delete(&self, keys: Vec<Vec<u8>>) -> KvResult<()> {
        for key in keys {
            self.set(key, None)?;
        }
        Ok(())
    }
    async fn put_version(
        &self,
        key: &[u8],
        from: i64,
        to: i64,
        tx: i64,
        value: &[u8],
    ) -> KvResult<()> {
        self.batch_put_version(vec![(key.to_vec(), from, to, tx, value.to_vec())])
            .await
    }
    async fn batch_put_version(&self, versions: Vec<VersionRow>) -> KvResult<()> {
        let mut changes = self.changes.lock();
        let total = versions
            .iter()
            .fold(changes.bytes, |size, (key, _, _, _, value)| {
                size.saturating_add(key.len() + value.len() + 64)
            });
        self.check_size(total)?;
        changes.bytes = total;
        changes.versions.extend(versions);
        Ok(())
    }
    async fn scan_history(&self, key: &[u8]) -> KvResult<Vec<VersionRecord>> {
        let mut rows = self.base.scan_history(key).await?;
        rows.extend(
            self.changes
                .lock()
                .versions
                .iter()
                .filter(|(entity, _, _, _, _)| entity == key)
                .map(|(_, from, to, tx, value)| VersionRecord {
                    valid_from: *from,
                    valid_to: *to,
                    tx: *tx,
                    value: value.clone(),
                }),
        );
        rows.sort_by_key(|row| std::cmp::Reverse((row.valid_from, row.tx)));
        Ok(rows)
    }
    async fn scan_history_entity_keys(&self, prefix: &[u8]) -> KvResult<Vec<Vec<u8>>> {
        let mut keys: BTreeSet<_> = self
            .base
            .scan_history_entity_keys(prefix)
            .await?
            .into_iter()
            .collect();
        keys.extend(
            self.changes
                .lock()
                .versions
                .iter()
                .filter(|(key, _, _, _, _)| key.starts_with(prefix))
                .map(|(key, _, _, _, _)| key.clone()),
        );
        Ok(keys.into_iter().collect())
    }
    async fn scan_prefix(&self, prefix: &[u8]) -> KvResult<Vec<(Vec<u8>, Vec<u8>)>> {
        let mut rows: BTreeMap<_, _> = self.base.scan_prefix(prefix).await?.into_iter().collect();
        for (key, value) in &self.changes.lock().values {
            if key.starts_with(prefix) {
                match value {
                    Some(value) => {
                        rows.insert(key.clone(), value.clone());
                    }
                    None => {
                        rows.remove(key);
                    }
                }
            }
        }
        Ok(rows.into_iter().collect())
    }
    async fn batch_get(&self, keys: &[Vec<u8>]) -> KvResult<Vec<Option<Vec<u8>>>> {
        let mut rows = Vec::with_capacity(keys.len());
        for key in keys {
            rows.push(self.get(key).await?);
        }
        Ok(rows)
    }
    async fn add_counters(&self, deltas: Vec<(Vec<u8>, i64)>) -> KvResult<()> {
        for (key, delta) in deltas {
            let current = match self.get(&key).await? {
                Some(bytes) => {
                    let encoded: [u8; 8] = bytes.try_into().map_err(|_| {
                        KVStoreError::Io(std::io::Error::other(
                            "Invalid degree counter: expected exactly 8 bytes; no statement writes committed",
                        ))
                    })?;
                    i64::from_le_bytes(encoded)
                }
                None => 0,
            };
            let updated = current.checked_add(delta).ok_or_else(|| {
                KVStoreError::Io(std::io::Error::other(
                    "Degree counter overflow or underflow; no statement writes committed",
                ))
            })?;
            // Preserve the existing <= 0 removal contract; only malformed
            // encodings and arithmetic outside i64 fail the statement.
            if updated <= 0 {
                self.delete(&key).await?;
            } else {
                self.put(&key, &updated.to_le_bytes()).await?;
            }
        }
        Ok(())
    }
    async fn batch_apply(
        &self,
        puts: Vec<(Vec<u8>, Vec<u8>)>,
        deletes: Vec<Vec<u8>>,
        versions: Vec<VersionRow>,
    ) -> KvResult<()> {
        self.batch_put(puts).await?;
        self.batch_delete(deletes).await?;
        self.batch_put_version(versions).await
    }
}

impl Executor {
    pub(super) async fn execute_dml_atomically(
        &self,
        plan: ExecutionPlan,
    ) -> Result<ExecutorResult> {
        let (space, _) = self
            .inference_scope(&plan)
            .ok_or_else(|| ExecutionError::InvalidOperation("No space selected".into()))?;
        let staged = Arc::new(MutationStore::new(
            self.ctx.kvstore.clone(),
            self.ctx.config.max_memory_mb,
        ));
        let mut ctx = self.ctx.derive_with_space(space.clone());
        if self.ctx.space.as_deref() == Some(&space) {
            ctx.space_id = self.ctx.space_id;
        }
        ctx.kvstore = staged.clone();
        // Never mutate the outer manager's cached maps while the transaction
        // is staged. This manager is discarded on commit or failure.
        ctx.index_manager = Some(Arc::new(IndexManager::new(staged.clone())));
        let executor = Executor::new(Arc::new(ctx));
        let result = match plan {
            ExecutionPlan::Insert(p) => executor.execute_insert(p).await?,
            ExecutionPlan::Update(p) => executor.execute_update(p).await?,
            ExecutionPlan::Delete(p) => executor.execute_delete(p).await?,
            ExecutionPlan::DeleteEdge(p) => executor.execute_delete_edge(p).await?,
            _ => {
                return Err(ExecutionError::InvalidOperation(
                    "Expected DML statement".into(),
                ))
            }
        };
        staged.commit().await?;
        Ok(result)
    }

    /// Mutation paths must not use the IndexManager read convenience methods:
    /// those log loading failures and return an empty list, which could commit
    /// graph writes without maintaining its indexes.
    pub(super) async fn mutation_indexes(
        &self,
        space: &str,
        kind: IndexType,
    ) -> Result<Vec<IndexDef>> {
        let space_id = if let Some(id) = self.ctx.space_id {
            id
        } else {
            match self
                .ctx
                .kvstore
                .get(&crate::key::SchemaKey::space(space))
                .await?
            {
                Some(bytes) => serde_json::from_slice::<serde_json::Value>(&bytes)?
                    .get("id")
                    .and_then(|v| v.as_u64())
                    .and_then(|v| u32::try_from(v).ok())
                    .ok_or_else(|| {
                        ExecutionError::InvalidOperation("Invalid space id metadata".into())
                    })?,
                None => 1,
            }
        };
        let prefix = format!("{}{}:", byoridb_storage::index::INDEX_DEF_PREFIX, space_id);
        let mut indexes = Vec::new();
        for (key, bytes) in self.ctx.kvstore.scan_prefix(prefix.as_bytes()).await? {
            let def: IndexDef = serde_json::from_slice(&bytes)?;
            if def.space_id != space_id
                || key != format!("{prefix}{}", def.id).as_bytes()
                || def.fields.is_empty()
                || def.schema_name.is_empty()
            {
                return Err(ExecutionError::InvalidOperation(
                    "Invalid persisted index definition".into(),
                ));
            }
            if def.index_type == kind {
                indexes.push(def);
            }
        }
        Ok(indexes)
    }
}

impl Executor {
    pub(super) async fn vertex_index_keys(
        &self,
        space: &str,
        vertex: &byoridb_codec::VertexData,
    ) -> Result<Vec<Vec<u8>>> {
        let mut keys = Vec::new();
        for index in self.mutation_indexes(space, IndexType::Tag).await? {
            if let Some(tag) = vertex.tags.iter().find(|tag| tag.name == index.schema_name) {
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
                keys.push(byoridb_storage::KeyUtils::tag_index_key(
                    1, index.id, &values, vertex.vid,
                ));
            }
        }
        Ok(keys)
    }
    pub(super) async fn edge_index_keys(
        &self,
        space: &str,
        edge: &byoridb_codec::EdgeData,
    ) -> Result<Vec<Vec<u8>>> {
        let mut keys = Vec::new();
        for index in self
            .mutation_indexes(space, IndexType::Edge)
            .await?
            .into_iter()
            .filter(|index| index.schema_name == edge.edge_type)
        {
            let values = index
                .fields
                .iter()
                .map(|field| {
                    self.byoridb_value_to_index_value(
                        edge.properties
                            .get(field)
                            .unwrap_or(&byoridb_common::Value::null()),
                    )
                })
                .collect::<Vec<_>>();
            keys.push(byoridb_storage::KeyUtils::edge_index_key(
                1,
                index.id,
                &values,
                edge.src_vid,
                edge.ranking,
                edge.dst_vid,
            ));
        }
        Ok(keys)
    }
}

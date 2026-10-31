//! Durable inference completion fence for standalone graph operations.
//!
//! A marker is committed with asserted graph/history writes. A successful
//! closure rebuild clears it. Cancellation, a crash, or any error leaves the
//! marker, so a new request/process cannot mistake partial derivation for a
//! complete graph. This is recovery, not rollback of committed assertions.
use super::{Executor, ExecutorResult};
use crate::error::{ExecutionError, Result};
use crate::plan::{AlterPlan, CreatePlan, DropPlan, ExecutionPlan, InsertPlan, ShowPlan};
use byoridb_common::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use tokio::sync::RwLock;

type SpaceLocks = HashMap<(usize, String), Weak<RwLock<()>>>;
static SPACE_LOCKS: OnceLock<Mutex<SpaceLocks>> = OnceLock::new();

impl Executor {
    pub(super) fn inference_dirty_key(space: &str) -> Vec<u8> {
        format!("{space}:inference:dirty").into_bytes()
    }

    pub(crate) fn inference_lock(&self, space: &str) -> Arc<RwLock<()>> {
        let store = Arc::as_ptr(&self.ctx.kvstore) as *const () as usize;
        let mut locks = SPACE_LOCKS.get_or_init(Mutex::default).lock().unwrap();
        locks.retain(|_, lock| lock.strong_count() != 0);
        locks
            .entry((store, space.to_owned()))
            .or_default()
            .upgrade()
            .unwrap_or_else(|| {
                let lock = Arc::new(RwLock::new(()));
                locks.insert((store, space.to_owned()), Arc::downgrade(&lock));
                lock
            })
    }

    pub(super) fn inference_scope(&self, plan: &ExecutionPlan) -> Option<(String, bool)> {
        let (explicit, mutation) = match plan {
            ExecutionPlan::Insert(
                InsertPlan::Vertex { space, .. } | InsertPlan::Edge { space, .. },
            ) => (Some(space), true),
            ExecutionPlan::Update(p) => (Some(&p.space), true),
            ExecutionPlan::Delete(p) => (Some(&p.space), true),
            ExecutionPlan::DeleteEdge(p) => (Some(&p.space), true),
            ExecutionPlan::Fetch(p) => (Some(&p.space), false),
            ExecutionPlan::Create(CreatePlan::Space { name, .. })
            | ExecutionPlan::Drop(DropPlan::Space { name, .. }) => (Some(name), true),
            ExecutionPlan::Create(
                CreatePlan::Tag { .. }
                | CreatePlan::Edge { .. }
                | CreatePlan::TagIndex { .. }
                | CreatePlan::EdgeIndex { .. }
                | CreatePlan::Class { .. }
                | CreatePlan::Shape { .. },
            )
            | ExecutionPlan::Alter(AlterPlan::Tag { .. } | AlterPlan::Edge { .. })
            | ExecutionPlan::Drop(
                DropPlan::Tag { .. }
                | DropPlan::Edge { .. }
                | DropPlan::TagIndex { .. }
                | DropPlan::EdgeIndex { .. }
                | DropPlan::Class { .. }
                | DropPlan::Shape { .. },
            )
            | ExecutionPlan::RepairInference => (None, true),
            ExecutionPlan::Match(_)
            | ExecutionPlan::Go(_)
            | ExecutionPlan::Lookup(_)
            | ExecutionPlan::Find(_)
            | ExecutionPlan::Recommend(_)
            | ExecutionPlan::CheckConsistency
            | ExecutionPlan::CheckShape
            | ExecutionPlan::ExplainInference { .. }
            | ExecutionPlan::CheckInference
            | ExecutionPlan::Show(ShowPlan::Stats) => (None, false),
            _ => return None,
        };
        explicit
            .filter(|s| !s.is_empty())
            .cloned()
            .or_else(|| self.ctx.space.clone())
            .map(|space| (space, mutation))
    }

    pub(super) async fn inference_is_dirty(&self, space: &str) -> Result<bool> {
        Ok(self
            .ctx
            .kvstore
            .get(&Self::inference_dirty_key(space))
            .await?
            .is_some())
    }

    pub(crate) async fn ensure_inference_clean(&self, space: &str) -> Result<()> {
        if self.inference_is_dirty(space).await? {
            return Err(ExecutionError::InvalidOperation(format!(
                "Inference incomplete in space '{space}'; assertions/history may already be committed. Run CHECK INFERENCE and have an administrator run REPAIR INFERENCE before reading or mutating this space."
            )));
        }
        Ok(())
    }

    /// Called immediately before an asserted graph/history batch is committed.
    /// Schema decoding errors occur before the batch, never silently disable the fence.
    pub(super) async fn fence_semantic_batch(
        &self,
        space: &str,
        puts: &mut Vec<(Vec<u8>, Vec<u8>)>,
    ) -> Result<()> {
        if !self.load_rel_meta(space).await?.is_empty() {
            puts.push((Self::inference_dirty_key(space), b"1".to_vec()));
        }
        Ok(())
    }

    pub(super) async fn check_inference(&self) -> Result<ExecutorResult> {
        let space = self.require_space()?;
        let dirty = self.inference_is_dirty(space).await?;
        Ok(ExecutorResult {
            columns: vec!["space".into(), "status".into()],
            rows: vec![vec![
                Value::String(space.to_owned()),
                Value::String(if dirty { "repair_required" } else { "complete" }.into()),
            ]],
            latency_ms: 0,
        })
    }

    pub(super) async fn repair_inference(&self) -> Result<ExecutorResult> {
        if !self.ctx.is_admin() {
            return Err(ExecutionError::InvalidOperation(
                "REPAIR INFERENCE requires GOD or ADMIN role".into(),
            ));
        }
        let space = self.require_space()?;
        self.ctx
            .kvstore
            .put(&Self::inference_dirty_key(space), b"1")
            .await?;
        self.rematerialize_space(space).await?;
        self.ctx
            .kvstore
            .delete(&Self::inference_dirty_key(space))
            .await?;
        self.check_inference().await
    }
}

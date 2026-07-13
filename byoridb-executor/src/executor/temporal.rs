// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! T-트랙 쓰기 경로 헬퍼 (bitemporal, asserted-facts-only).
//!
//! DML 은 현재뷰(`{space}:vertex:{vid}` / `{space}:edge:…`) 변경과 HISTORY_TABLE
//! 버전 append 를 `KVStore::batch_apply` **단일 트랜잭션**으로 커밋한다
//! (v1.1 ①: dual-write 원자성 — 중간 크래시로 현재값/이력이 어긋날 수 없다).
//!
//! v1 에서 valid-time 은 transaction-time 과 동일하며, 각 버전은 `[tx, ∞)` 오픈
//! 구간이라 resolution 이 "as-of 시점 이하 최신 valid_from" 을 고르면
//! point-in-time 이 자연히 성립한다. 삭제는 빈-payload tombstone 버전.
//! tx 는 단조증가(`max(벽시계, last+1)`)라 같은 엔티티의 같은 millisecond
//! 쓰기도 history key 가 충돌하지 않는다 (v1.1 ②).
//!
//! current view 는 기존 경로 그대로 → 읽기·추론(B) 무회귀 (design.md §5).

use super::Executor;
use byoridb_kvstore::VALID_OPEN;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Last transaction time handed out (epoch ms). See [`Executor::tx_now`].
static LAST_TX: AtomicI64 = AtomicI64::new(0);

/// One history version row: `(entity_key, valid_from, valid_to, tx, payload)`.
pub(super) type VersionRow = (Vec<u8>, i64, i64, i64, Vec<u8>);

impl Executor {
    /// Monotonic transaction time as epoch millis (D6): `max(벽시계, last+1)`.
    /// valid-time 도 v1 에선 동일 값. 지속적으로 1ms 에 1회 이상 쓰면 값이
    /// 벽시계보다 잠시 앞서지만, 시계가 따라잡는 즉시 재동기화된다.
    pub(super) fn tx_now() -> i64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let mut last = LAST_TX.load(Ordering::Relaxed);
        loop {
            let next = now.max(last + 1);
            match LAST_TX.compare_exchange_weak(last, next, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => return next,
                Err(observed) => last = observed,
            }
        }
    }

    /// `(현재뷰 키, payload)` 쌍들을 `[tx, ∞)@tx` 버전 행으로. 행마다 고유한
    /// 단조 tx 를 받아 같은 엔티티가 한 배치에 두 번 있어도 키가 충돌하지 않는다.
    pub(super) fn build_versions(entities: Vec<(Vec<u8>, Vec<u8>)>) -> Vec<VersionRow> {
        entities
            .into_iter()
            .map(|(k, v)| {
                let tx = Self::tx_now();
                (k, tx, VALID_OPEN, tx, v)
            })
            .collect()
    }

    /// 삭제 tombstone 버전 행들 (빈 payload = `tx` 시점 이후 부재).
    /// 읽기 경로는 빈 payload 를 "그 시점엔 존재하지 않음"으로 해석한다(T-3).
    pub(super) fn build_tombstones(entity_keys: Vec<Vec<u8>>) -> Vec<VersionRow> {
        entity_keys
            .into_iter()
            .map(|k| {
                let tx = Self::tx_now();
                (k, tx, VALID_OPEN, tx, Vec::new())
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::context::ExecutionContext;
    use crate::executor::Executor;
    use byoridb_kvstore::{KVStore, MemoryKVStore};
    use std::sync::Arc;

    fn create() -> (Executor, Arc<MemoryKVStore>) {
        let kv = Arc::new(MemoryKVStore::new());
        let ctx = Arc::new(ExecutionContext::new(kv.clone()).with_space("default".to_string()));
        (Executor::new(ctx), kv)
    }

    async fn run(executor: &Executor, q: &str) -> crate::executor::ExecutorResult {
        let stmt = byoridb_parser::parse(q).expect("parse");
        let plan = crate::ExecutionPlanBuilder::build(stmt).expect("plan build");
        executor
            .execute(plan)
            .await
            .unwrap_or_else(|e| panic!("query failed: {q}\n{e:?}"))
    }

    fn row_text(r: &crate::executor::ExecutorResult) -> String {
        format!("{:?}", r.rows)
    }

    /// v1.1 ②: 같은 millisecond 의 연속 쓰기도 각자 이력 행을 남긴다
    /// (단조 tx 가 history key 충돌을 제거).
    #[tokio::test]
    async fn same_millisecond_writes_keep_distinct_history_rows() {
        let (e, kv) = create();
        run(&e, "CREATE TAG note(body STRING)").await;
        run(&e, "INSERT VERTEX note(body) VALUES 2:(\"x\")").await;
        run(&e, "INSERT VERTEX note(body) VALUES 2:(\"y\")").await;
        run(&e, "UPDATE VERTEX ON note 2 SET body = \"z\"").await;

        let hist = kv.scan_history(b"default:vertex:2").await.unwrap();
        assert_eq!(hist.len(), 3, "매 쓰기가 고유 버전이어야 함 (ms 충돌 없음)");
        assert!(
            hist[0].tx > hist[1].tx && hist[1].tx > hist[2].tx,
            "단조 tx: {:?}",
            hist.iter().map(|v| v.tx).collect::<Vec<_>>()
        );
    }

    /// v1.1 ④: public DML → parse/plan → `FETCH … AS OF` end-to-end.
    /// 시각은 벽시계가 아니라 실제 기록된 이력에서 읽는다(단조 tx 와 무관하게 안전).
    #[tokio::test]
    async fn dml_to_fetch_as_of_end_to_end() {
        let (e, kv) = create();
        run(&e, "CREATE TAG note(body STRING)").await;
        run(&e, "INSERT VERTEX note(body) VALUES 1:(\"a\")").await;
        run(&e, "UPDATE VERTEX ON note 1 SET body = \"b\"").await;

        let hist = kv.scan_history(b"default:vertex:1").await.unwrap();
        assert_eq!(hist.len(), 2);
        let (t2, t1) = (hist[0].tx, hist[1].tx); // newest-first

        let r = run(&e, &format!("FETCH PROP ON note 1 AS OF {t1}")).await;
        assert_eq!(r.rows.len(), 1);
        assert!(row_text(&r).contains('a'), "t1 시점은 최초값: {:?}", r.rows);

        let r = run(&e, &format!("FETCH PROP ON note 1 AS OF {t2}")).await;
        assert_eq!(r.rows.len(), 1);
        assert!(row_text(&r).contains('b'), "t2 시점은 갱신값: {:?}", r.rows);

        // DELETE → tombstone: 그 시점 이후 부재, 과거는 그대로.
        run(&e, "DELETE VERTEX 1").await;
        let hist = kv.scan_history(b"default:vertex:1").await.unwrap();
        assert_eq!(hist.len(), 3);
        let t3 = hist[0].tx;
        assert!(hist[0].value.is_empty(), "tombstone 은 빈 payload");

        let r = run(&e, &format!("FETCH PROP ON note 1 AS OF {t3}")).await;
        assert_eq!(r.rows.len(), 0, "삭제 시점 이후엔 부재");
        let r = run(&e, &format!("FETCH PROP ON note 1 AS OF {t2}")).await;
        assert_eq!(r.rows.len(), 1, "삭제 전 과거는 계속 조회 가능");
        let r = run(&e, "FETCH PROP ON note 1").await;
        assert_eq!(r.rows.len(), 0, "현재뷰에서도 삭제됨");
    }

    /// v1.1 ①: DML 의 현재뷰 변경과 이력 append 는 항상 짝을 이룬다
    /// (batch_apply 단일 트랜잭션 — 엣지 쓰기/삭제 경로 포함).
    #[tokio::test]
    async fn edge_writes_pair_current_view_with_history() {
        let (e, kv) = create();
        run(&e, "CREATE TAG t(name STRING)").await;
        run(&e, "CREATE EDGE rel(kind STRING)").await;
        run(&e, "INSERT VERTEX t(name) VALUES 1:(\"a\")").await;
        run(&e, "INSERT VERTEX t(name) VALUES 2:(\"b\")").await;
        run(&e, "INSERT EDGE rel(kind) VALUES 1->2:(\"r\")").await;

        let ekey = b"default:edge:1:rel:2:0".as_slice();
        assert!(kv.get(ekey).await.unwrap().is_some(), "현재뷰 엣지");
        assert_eq!(kv.scan_history(ekey).await.unwrap().len(), 1, "이력 1행");

        run(&e, "DELETE EDGE rel 1->2").await;
        assert!(kv.get(ekey).await.unwrap().is_none(), "현재뷰에서 제거");
        let hist = kv.scan_history(ekey).await.unwrap();
        assert_eq!(hist.len(), 2, "tombstone 추가");
        assert!(hist[0].value.is_empty());
    }
}

// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! T-트랙 쓰기 경로 헬퍼 (bitemporal, asserted-facts-only).
//!
//! 현재뷰(`{space}:vertex:{vid}` / `{space}:edge:…`) 쓰기와 함께, 물리적으로
//! 분리된 HISTORY_TABLE 에 버전을 append 한다. v1 에서 valid-time 은
//! transaction-time 과 동일(=now)하며, 각 버전은 `[now, ∞)` 오픈 구간이라
//! resolution 이 "as-of 시점 이하 최신 valid_from" 을 고르면 point-in-time 이
//! 자연히 성립한다(구간을 닫을 필요 없음). 삭제는 빈-payload tombstone 버전.
//!
//! current view 는 기존 경로 그대로 → 읽기·추론(B) 무회귀 (design.md §5, D-scope-3).

use super::Executor;
use crate::error::Result;
use byoridb_kvstore::VALID_OPEN;
use std::time::{SystemTime, UNIX_EPOCH};

impl Executor {
    /// Transaction time as epoch millis (D6). valid-time 도 v1 에선 동일 값.
    pub(super) fn tx_now() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    /// 현재값 blob 들을 한 트랜잭션으로 이력에 append (`[now, ∞)@now`).
    /// `entities` = `(현재뷰 키 바이트, 인코딩된 payload)` 목록.
    pub(super) async fn record_versions(&self, entities: Vec<(Vec<u8>, Vec<u8>)>) -> Result<()> {
        if entities.is_empty() {
            return Ok(());
        }
        let tx = Self::tx_now();
        let versions = entities
            .into_iter()
            .map(|(k, v)| (k, tx, VALID_OPEN, tx, v))
            .collect();
        self.ctx.kvstore.batch_put_version(versions).await?;
        Ok(())
    }

    /// 삭제 tombstone 버전들 append (빈 payload = `now` 시점 이후 무효).
    /// 읽기 경로는 빈 payload 를 "그 시점엔 존재하지 않음"으로 해석한다(T-3).
    pub(super) async fn record_tombstones(&self, entity_keys: Vec<Vec<u8>>) -> Result<()> {
        if entity_keys.is_empty() {
            return Ok(());
        }
        let tx = Self::tx_now();
        let versions = entity_keys
            .into_iter()
            .map(|k| (k, tx, VALID_OPEN, tx, Vec::new()))
            .collect();
        self.ctx.kvstore.batch_put_version(versions).await?;
        Ok(())
    }
}

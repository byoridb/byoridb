// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Tokenized text search over vertex string properties.
//!
//! This is deliberately separate from the scalar tag index. Product-name search
//! needs multiple tokens per document, model-code weighting, and ranked results
//! rather than exact equality on one value.

use super::{Executor, ExecutorResult};
use crate::error::{ExecutionError, Result};
use crate::plan::{SearchPlan, TextIndexPlan};
use byoridb_codec::{VertexCodec, VertexData};
use byoridb_common::Value;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use tracing::info;

const DEFAULT_BATCH: usize = 4096;
const MAX_TOKEN_LEN: usize = 96;
const REBUILD_PROGRESS_INTERVAL: u64 = 100_000;
const RESUME_DOC_PROGRESS_INTERVAL: usize = 1_000_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TextDoc {
    len: u32,
    text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TextStats {
    docs: u64,
    total_len: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct TextIndexSpec {
    pub tag_name: String,
    pub prop: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct TextIndexManifest {
    indexes: Vec<TextIndexSpec>,
}

#[derive(Debug, Clone)]
struct CandidateTerm {
    tf: u16,
    idf: f64,
    query_weight: u16,
}

impl Executor {
    pub(super) async fn execute_rebuild_text_index(
        &self,
        plan: TextIndexPlan,
    ) -> Result<ExecutorResult> {
        let space = self.require_space()?.to_string();
        let prefix = text_index_prefix(&space, &plan.tag_name, &plan.prop);
        let stats_key = text_stats_key(&space, &plan.tag_name, &plan.prop);
        let existing_docs = if self.ctx.kvstore.get(&stats_key).await?.is_none() {
            self.load_partial_text_doc_ids(&space, &plan.tag_name, &plan.prop)
                .await?
        } else {
            HashSet::new()
        };
        let resume_existing_docs = existing_docs.len();
        let deleted = if resume_existing_docs == 0 {
            self.ctx
                .kvstore
                .delete_prefix_chunked(&prefix, 10_000)
                .await?
        } else {
            info!(
                space = %space,
                tag = %plan.tag_name,
                prop = %plan.prop,
                resume_existing_docs,
                "text index rebuild resume detected"
            );
            0
        };

        let mut stream = self
            .ctx
            .kvstore
            .scan_stream(&crate::key::SchemaKey::vertex_prefix(&space))
            .await?;
        let mut candidates: Vec<(i64, String)> = Vec::new();
        let mut scanned = 0u64;
        let mut candidate_docs = 0u64;

        info!(
            space = %space,
            tag = %plan.tag_name,
            prop = %plan.prop,
            deleted,
            resume_existing_docs,
            "text index rebuild scan started"
        );

        while let Some(item) = stream.next().await {
            scanned += 1;
            if scanned.is_multiple_of(REBUILD_PROGRESS_INTERVAL) {
                info!(
                    space = %space,
                    tag = %plan.tag_name,
                    prop = %plan.prop,
                    scanned,
                    candidate_docs,
                    "text index rebuild scan progress"
                );
            }

            let (_key, value) = item?;
            let Ok(vertex) = VertexCodec::decode_vertex(&value) else {
                continue;
            };
            let Some(tag) = vertex.tags.iter().find(|tag| tag.name == plan.tag_name) else {
                continue;
            };
            let Some(Value::String(text)) = tag.properties.get(&plan.prop) else {
                continue;
            };
            if text.trim().is_empty() {
                continue;
            }
            candidate_docs += 1;
            candidates.push((vertex.vid, text.clone()));
        }
        drop(stream);

        info!(
            space = %space,
            tag = %plan.tag_name,
            prop = %plan.prop,
            scanned,
            candidate_docs,
            "text index rebuild scan complete"
        );

        let mut batch: Vec<(Vec<u8>, Vec<u8>)> = Vec::with_capacity(DEFAULT_BATCH);
        let mut docs = 0u64;
        let mut total_len = 0u64;
        let mut postings = 0u64;
        let mut resumed_docs = 0u64;
        let mut written_docs = 0u64;

        for (vid, text) in candidates {
            let Some((doc, terms)) = indexed_doc(&text) else {
                continue;
            };

            docs += 1;
            total_len += u64::from(doc.len);
            postings += terms.len() as u64;

            if existing_docs.contains(&vid) {
                resumed_docs += 1;
                if docs.is_multiple_of(REBUILD_PROGRESS_INTERVAL) {
                    info!(
                        space = %space,
                        tag = %plan.tag_name,
                        prop = %plan.prop,
                        docs,
                        postings,
                        resumed_docs,
                        written_docs,
                        "text index rebuild write progress"
                    );
                }
                continue;
            }

            written_docs += 1;
            batch.push((
                text_doc_key(&space, &plan.tag_name, &plan.prop, vid),
                serde_json::to_vec(&doc)?,
            ));

            for (term, freq) in terms {
                batch.push((
                    text_posting_key(&space, &plan.tag_name, &plan.prop, &term, vid),
                    freq.to_le_bytes().to_vec(),
                ));
                if batch.len() >= DEFAULT_BATCH {
                    self.ctx
                        .kvstore
                        .batch_put(std::mem::take(&mut batch))
                        .await?;
                }
            }

            if docs.is_multiple_of(REBUILD_PROGRESS_INTERVAL) {
                info!(
                    space = %space,
                    tag = %plan.tag_name,
                    prop = %plan.prop,
                    docs,
                    postings,
                    resumed_docs,
                    written_docs,
                    "text index rebuild write progress"
                );
            }
        }

        batch.push((
            text_stats_key(&space, &plan.tag_name, &plan.prop),
            serde_json::to_vec(&TextStats { docs, total_len })?,
        ));
        if !batch.is_empty() {
            self.ctx.kvstore.batch_put(batch).await?;
        }
        self.add_text_index_to_manifest(&space, &plan.tag_name, &plan.prop)
            .await?;

        Ok(ExecutorResult {
            columns: vec![
                "indexed_docs".to_string(),
                "postings".to_string(),
                "deleted_keys".to_string(),
            ],
            rows: vec![vec![
                Value::Int(docs as i64),
                Value::Int(postings as i64),
                Value::Int(deleted as i64),
            ]],
            latency_ms: 0,
        })
    }

    pub(super) async fn execute_search(&self, plan: SearchPlan) -> Result<ExecutorResult> {
        let space = self.require_space()?.to_string();
        let Some(stats_bytes) = self
            .ctx
            .kvstore
            .get(&text_stats_key(&space, &plan.tag_name, &plan.prop))
            .await?
        else {
            return Err(ExecutionError::InvalidOperation(format!(
                "text index on {}.{} is not built; run REBUILD TEXT INDEX ON {}({})",
                plan.tag_name, plan.prop, plan.tag_name, plan.prop
            )));
        };
        let stats: TextStats = serde_json::from_slice(&stats_bytes)?;
        if stats.docs == 0 {
            return Ok(search_result(Vec::new()));
        }

        let query_terms = weighted_terms(&plan.query);
        if query_terms.is_empty() {
            return Ok(search_result(Vec::new()));
        }

        let mut candidates: HashMap<i64, Vec<CandidateTerm>> = HashMap::new();
        for (term, query_weight) in query_terms {
            let mut stream = self
                .ctx
                .kvstore
                .scan_stream(&text_posting_prefix(
                    &space,
                    &plan.tag_name,
                    &plan.prop,
                    &term,
                ))
                .await?;
            let mut postings = Vec::new();
            let mut scanned = 0usize;
            while let Some(item) = stream.next().await {
                let (key, value) = item?;
                let Some(vid) = vid_from_posting_key(&key) else {
                    continue;
                };
                let tf = freq_from_bytes(&value);
                if tf == 0 {
                    continue;
                }
                postings.push((vid, tf));
                scanned += 1;
                if scanned >= self.ctx.config.max_scan_limit {
                    tracing::warn!(
                        term,
                        cap = self.ctx.config.max_scan_limit,
                        "text search posting scan reached max_scan_limit"
                    );
                    break;
                }
            }
            if postings.is_empty() {
                continue;
            }
            let df = postings.len() as f64;
            let idf = ((stats.docs as f64 - df + 0.5) / (df + 0.5))
                .max(0.0)
                .ln_1p()
                + 1.0;
            for (vid, tf) in postings {
                candidates.entry(vid).or_default().push(CandidateTerm {
                    tf,
                    idf,
                    query_weight,
                });
            }
        }

        let avgdl = (stats.total_len as f64 / stats.docs as f64).max(1.0);
        let mut scored = Vec::with_capacity(candidates.len());
        for (vid, terms) in candidates {
            let Some(doc_bytes) = self
                .ctx
                .kvstore
                .get(&text_doc_key(&space, &plan.tag_name, &plan.prop, vid))
                .await?
            else {
                continue;
            };
            let doc: TextDoc = serde_json::from_slice(&doc_bytes)?;
            let score = bm25_score(&terms, doc.len as f64, avgdl);
            scored.push((vid, score, terms.len() as i64, doc.text));
        }

        scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        scored.truncate(plan.limit);
        Ok(search_result(scored))
    }

    pub(super) async fn list_text_indexes(&self, space: &str) -> Result<Vec<TextIndexSpec>> {
        let Some(bytes) = self.ctx.kvstore.get(&text_manifest_key(space)).await? else {
            return Ok(Vec::new());
        };
        let manifest: TextIndexManifest = serde_json::from_slice(&bytes)?;
        Ok(manifest.indexes)
    }

    async fn load_partial_text_doc_ids(
        &self,
        space: &str,
        tag: &str,
        prop: &str,
    ) -> Result<HashSet<i64>> {
        let mut stream = self
            .ctx
            .kvstore
            .scan_stream(&text_doc_prefix(space, tag, prop))
            .await?;
        let mut docs = HashSet::new();

        while let Some(item) = stream.next().await {
            let (key, _value) = item?;
            let Some(vid) = vid_from_doc_key(&key) else {
                continue;
            };
            docs.insert(vid);
            if docs.len().is_multiple_of(RESUME_DOC_PROGRESS_INTERVAL) {
                info!(
                    space = %space,
                    tag = %tag,
                    prop = %prop,
                    existing_docs = docs.len(),
                    "text index rebuild resume scan progress"
                );
            }
        }

        Ok(docs)
    }

    pub(super) async fn sync_text_indexes_for_vertex(
        &self,
        space: &str,
        old_vertex: Option<&VertexData>,
        new_vertex: Option<&VertexData>,
        indexes: &[TextIndexSpec],
    ) -> Result<()> {
        if indexes.is_empty() {
            return Ok(());
        }

        for index in indexes {
            let old_text = vertex_text(old_vertex, index);
            let new_text = vertex_text(new_vertex, index);
            if old_text == new_text {
                continue;
            }

            let Some(mut stats) = self.load_text_stats(space, index).await? else {
                continue;
            };
            let old_doc = old_text.and_then(indexed_doc);
            let new_doc = new_text.and_then(indexed_doc);
            if old_doc.is_none() && new_doc.is_none() {
                continue;
            }

            if let Some((doc, terms)) = &old_doc {
                let mut keys = Vec::with_capacity(terms.len() + 1);
                let vid = old_vertex.map(|v| v.vid).unwrap_or_default();
                keys.push(text_doc_key(space, &index.tag_name, &index.prop, vid));
                for term in terms.keys() {
                    keys.push(text_posting_key(
                        space,
                        &index.tag_name,
                        &index.prop,
                        term,
                        vid,
                    ));
                }
                self.ctx.kvstore.batch_delete(keys).await?;
                stats.docs = stats.docs.saturating_sub(1);
                stats.total_len = stats.total_len.saturating_sub(u64::from(doc.len));
            }

            if let Some((doc, terms)) = &new_doc {
                let vid = new_vertex.map(|v| v.vid).unwrap_or_default();
                let mut pairs = Vec::with_capacity(terms.len() + 1);
                pairs.push((
                    text_doc_key(space, &index.tag_name, &index.prop, vid),
                    serde_json::to_vec(doc)?,
                ));
                for (term, freq) in terms {
                    pairs.push((
                        text_posting_key(space, &index.tag_name, &index.prop, term, vid),
                        freq.to_le_bytes().to_vec(),
                    ));
                }
                self.ctx.kvstore.batch_put(pairs).await?;
                stats.docs = stats.docs.saturating_add(1);
                stats.total_len = stats.total_len.saturating_add(u64::from(doc.len));
            }

            self.ctx
                .kvstore
                .put(
                    &text_stats_key(space, &index.tag_name, &index.prop),
                    &serde_json::to_vec(&stats)?,
                )
                .await?;
        }

        Ok(())
    }

    async fn load_text_stats(
        &self,
        space: &str,
        index: &TextIndexSpec,
    ) -> Result<Option<TextStats>> {
        let Some(bytes) = self
            .ctx
            .kvstore
            .get(&text_stats_key(space, &index.tag_name, &index.prop))
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_slice(&bytes)?))
    }

    async fn add_text_index_to_manifest(
        &self,
        space: &str,
        tag_name: &str,
        prop: &str,
    ) -> Result<()> {
        let mut indexes = self.list_text_indexes(space).await?;
        let spec = TextIndexSpec {
            tag_name: tag_name.to_string(),
            prop: prop.to_string(),
        };
        if !indexes.contains(&spec) {
            indexes.push(spec);
            indexes.sort_by(|a, b| {
                a.tag_name
                    .cmp(&b.tag_name)
                    .then_with(|| a.prop.cmp(&b.prop))
            });
            self.ctx
                .kvstore
                .put(
                    &text_manifest_key(space),
                    &serde_json::to_vec(&TextIndexManifest { indexes })?,
                )
                .await?;
        }
        Ok(())
    }
}

fn bm25_score(terms: &[CandidateTerm], doc_len: f64, avgdl: f64) -> f64 {
    const K1: f64 = 1.2;
    const B: f64 = 0.75;
    terms
        .iter()
        .map(|t| {
            let tf = f64::from(t.tf);
            let denom = tf + K1 * (1.0 - B + B * doc_len / avgdl);
            t.idf * (tf * (K1 + 1.0) / denom) * f64::from(t.query_weight)
        })
        .sum()
}

fn search_result(rows: Vec<(i64, f64, i64, String)>) -> ExecutorResult {
    ExecutorResult {
        columns: vec![
            "vid".to_string(),
            "score".to_string(),
            "matched_terms".to_string(),
            "text".to_string(),
        ],
        rows: rows
            .into_iter()
            .map(|(vid, score, matched, text)| {
                vec![
                    Value::Int(vid),
                    Value::Float(score),
                    Value::Int(matched),
                    Value::String(text),
                ]
            })
            .collect(),
        latency_ms: 0,
    }
}

fn vertex_text<'a>(vertex: Option<&'a VertexData>, index: &TextIndexSpec) -> Option<&'a str> {
    let vertex = vertex?;
    let tag = vertex.tags.iter().find(|tag| tag.name == index.tag_name)?;
    match tag.properties.get(&index.prop) {
        Some(Value::String(text)) => Some(text.as_str()),
        _ => None,
    }
}

fn indexed_doc(text: &str) -> Option<(TextDoc, HashMap<String, u16>)> {
    let terms = weighted_terms(text);
    if terms.is_empty() {
        return None;
    }
    let len = terms.values().map(|v| *v as u32).sum();
    Some((
        TextDoc {
            len,
            text: text.to_string(),
        },
        terms,
    ))
}

fn weighted_terms(text: &str) -> HashMap<String, u16> {
    let mut out = HashMap::new();
    for raw in lexical_tokens(text) {
        add_term(&mut out, &raw, token_weight(&raw));
        if raw.contains('_') || raw.contains('-') {
            for part in raw.split(['_', '-']).filter(|part| part.len() >= 2) {
                add_term(&mut out, part, token_weight(part));
            }
        }
    }
    out
}

fn lexical_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '_' || ch == '-' {
            for lower in ch.to_lowercase() {
                cur.push(lower);
            }
            if cur.chars().count() >= MAX_TOKEN_LEN {
                push_token(&mut tokens, &mut cur);
            }
        } else {
            push_token(&mut tokens, &mut cur);
        }
    }
    push_token(&mut tokens, &mut cur);
    tokens
}

fn push_token(tokens: &mut Vec<String>, cur: &mut String) {
    let trimmed = cur.trim_matches(['_', '-']);
    if trimmed.chars().count() >= 2 {
        tokens.push(trimmed.to_string());
    }
    cur.clear();
}

fn token_weight(token: &str) -> u16 {
    let has_digit = token.chars().any(|c| c.is_ascii_digit());
    let has_alpha = token.chars().any(|c| c.is_ascii_alphabetic());
    if token.contains('_') || token.contains('-') {
        10
    } else if has_digit && has_alpha && token.len() >= 4 {
        8
    } else {
        1
    }
}

fn add_term(out: &mut HashMap<String, u16>, term: &str, weight: u16) {
    if term.is_empty() {
        return;
    }
    let entry = out.entry(term.to_string()).or_insert(0);
    *entry = entry.saturating_add(weight);
}

fn freq_from_bytes(bytes: &[u8]) -> u16 {
    bytes
        .get(..2)
        .and_then(|b| b.try_into().ok())
        .map(u16::from_le_bytes)
        .unwrap_or(0)
}

fn text_index_prefix(space: &str, tag: &str, prop: &str) -> Vec<u8> {
    format!("{space}:textidx:{tag}:{prop}:").into_bytes()
}

fn text_manifest_key(space: &str) -> Vec<u8> {
    format!("{space}:textidx:manifest").into_bytes()
}

fn text_stats_key(space: &str, tag: &str, prop: &str) -> Vec<u8> {
    format!("{space}:textidx:{tag}:{prop}:stats").into_bytes()
}

fn text_doc_key(space: &str, tag: &str, prop: &str, vid: i64) -> Vec<u8> {
    format!("{space}:textidx:{tag}:{prop}:doc:{vid}").into_bytes()
}

fn text_doc_prefix(space: &str, tag: &str, prop: &str) -> Vec<u8> {
    format!("{space}:textidx:{tag}:{prop}:doc:").into_bytes()
}

fn text_posting_prefix(space: &str, tag: &str, prop: &str, term: &str) -> Vec<u8> {
    format!("{space}:textidx:{tag}:{prop}:post:{term}:").into_bytes()
}

fn text_posting_key(space: &str, tag: &str, prop: &str, term: &str, vid: i64) -> Vec<u8> {
    format!("{space}:textidx:{tag}:{prop}:post:{term}:{vid}").into_bytes()
}

fn vid_from_posting_key(key: &[u8]) -> Option<i64> {
    let s = std::str::from_utf8(key).ok()?;
    s.rsplit(':').next()?.parse().ok()
}

fn vid_from_doc_key(key: &[u8]) -> Option<i64> {
    let s = std::str::from_utf8(key).ok()?;
    s.rsplit(':').next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ExecutionContext;
    use crate::{ExecutionPlanBuilder, Executor};
    use byoridb_kvstore::store::MemoryKVStore;
    use std::sync::Arc;

    fn executor() -> Executor {
        let ctx = Arc::new(
            ExecutionContext::new(Arc::new(MemoryKVStore::new())).with_space("default".into()),
        );
        Executor::new(ctx)
    }

    async fn run(executor: &Executor, q: &str) -> Result<ExecutorResult> {
        let stmt = byoridb_parser::parse(q).expect("parse");
        let plan = ExecutionPlanBuilder::build(stmt).expect("plan");
        executor.execute(plan).await
    }

    #[test]
    fn tokenizer_weights_model_codes() {
        let terms = weighted_terms("[노스페이스] 세미 레인부츠 NS84S03B_PAP");
        assert!(terms.contains_key("노스페이스"));
        assert!(terms.contains_key("레인부츠"));
        assert!(terms.contains_key("ns84s03b_pap"));
        assert!(terms.contains_key("ns84s03b"));
        assert!(terms.get("ns84s03b_pap").copied().unwrap_or(0) > 1);
    }

    #[tokio::test]
    async fn rebuild_and_search_product_names() {
        let e = executor();
        run(&e, "CREATE TAG product(prod_name STRING, channel STRING)")
            .await
            .unwrap();
        run(
            &e,
            "INSERT VERTEX product(prod_name, channel) VALUES \
             1:(\"[노스페이스] 세미 레인부츠 NS84S03B_PAP\", \"naver\"), \
             2:(\"노스페이스 레인 부츠 NS84S03B PAP\", \"coupang\"), \
             3:(\"아디다스 운동화 ABC\", \"gmarket\")",
        )
        .await
        .unwrap();

        let rebuilt = run(&e, "REBUILD TEXT INDEX ON product(prod_name)")
            .await
            .unwrap();
        assert_eq!(rebuilt.rows[0][0], Value::Int(3));

        let result = run(
            &e,
            "SEARCH product.prod_name FOR \"[노스페이스] 세미 레인부츠 NS84S03B_PAP\" LIMIT 2",
        )
        .await
        .unwrap();
        assert_eq!(
            result.columns,
            vec!["vid", "score", "matched_terms", "text"]
        );
        assert_eq!(result.rows.len(), 2);
        assert_eq!(result.rows[0][0], Value::Int(1));
        assert_eq!(result.rows[1][0], Value::Int(2));
    }

    #[tokio::test]
    async fn rebuild_resumes_partial_text_index() {
        let e = executor();
        run(&e, "CREATE TAG product(prod_name STRING)")
            .await
            .unwrap();
        run(
            &e,
            "INSERT VERTEX product(prod_name) VALUES \
             1:(\"노스페이스 세미 레인부츠 NS84S03B_PAP\"), \
             2:(\"노스페이스 세미 레인부츠 NS84S03B_RED\")",
        )
        .await
        .unwrap();

        let text = "노스페이스 세미 레인부츠 NS84S03B_PAP";
        let (doc, terms) = indexed_doc(text).unwrap();
        let mut pairs = vec![(
            text_doc_key("default", "product", "prod_name", 1),
            serde_json::to_vec(&doc).unwrap(),
        )];
        for (term, freq) in terms {
            pairs.push((
                text_posting_key("default", "product", "prod_name", &term, 1),
                freq.to_le_bytes().to_vec(),
            ));
        }
        e.ctx.kvstore.batch_put(pairs).await.unwrap();

        let rebuilt = run(&e, "REBUILD TEXT INDEX ON product(prod_name)")
            .await
            .unwrap();
        assert_eq!(rebuilt.rows[0][0], Value::Int(2));
        assert_eq!(rebuilt.rows[0][2], Value::Int(0));

        let existing = run(&e, "SEARCH product.prod_name FOR 'NS84S03B_PAP' LIMIT 1")
            .await
            .unwrap();
        assert_eq!(existing.rows.len(), 1);
        assert_eq!(existing.rows[0][0], Value::Int(1));

        let written = run(&e, "SEARCH product.prod_name FOR 'NS84S03B_RED' LIMIT 1")
            .await
            .unwrap();
        assert_eq!(written.rows.len(), 1);
        assert_eq!(written.rows[0][0], Value::Int(2));
    }

    #[tokio::test]
    async fn insert_after_rebuild_updates_text_index_incrementally() {
        let e = executor();
        run(&e, "CREATE TAG product(prod_name STRING)")
            .await
            .unwrap();
        run(
            &e,
            "INSERT VERTEX product(prod_name) VALUES 1:(\"나이키 운동화 AAA111\")",
        )
        .await
        .unwrap();
        run(&e, "REBUILD TEXT INDEX ON product(prod_name)")
            .await
            .unwrap();
        run(
            &e,
            "INSERT VERTEX product(prod_name) VALUES \
             2:(\"[노스페이스] 세미 레인부츠 NS84S03B_PAP\")",
        )
        .await
        .unwrap();

        let result = run(&e, "SEARCH product.prod_name FOR 'NS84S03B_PAP' LIMIT 10")
            .await
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0][0], Value::Int(2));
    }

    #[tokio::test]
    async fn update_after_rebuild_replaces_text_index_terms() {
        let e = executor();
        run(&e, "CREATE TAG product(prod_name STRING)")
            .await
            .unwrap();
        run(
            &e,
            "INSERT VERTEX product(prod_name) VALUES 1:(\"OLD123_X 이전 상품\")",
        )
        .await
        .unwrap();
        run(&e, "REBUILD TEXT INDEX ON product(prod_name)")
            .await
            .unwrap();
        run(
            &e,
            "UPDATE VERTEX ON product 1 SET prod_name = \"NEW456_Y 신규 상품\"",
        )
        .await
        .unwrap();

        let old = run(&e, "SEARCH product.prod_name FOR 'OLD123_X' LIMIT 10")
            .await
            .unwrap();
        assert!(old.rows.is_empty());

        let new = run(&e, "SEARCH product.prod_name FOR 'NEW456_Y' LIMIT 10")
            .await
            .unwrap();
        assert_eq!(new.rows.len(), 1);
        assert_eq!(new.rows[0][0], Value::Int(1));
    }

    #[tokio::test]
    async fn delete_after_rebuild_removes_text_index_terms() {
        let e = executor();
        run(&e, "CREATE TAG product(prod_name STRING)")
            .await
            .unwrap();
        run(
            &e,
            "INSERT VERTEX product(prod_name) VALUES 1:(\"DELETE999_Z 삭제 상품\")",
        )
        .await
        .unwrap();
        run(&e, "REBUILD TEXT INDEX ON product(prod_name)")
            .await
            .unwrap();
        run(&e, "DELETE VERTEX 1").await.unwrap();

        let result = run(&e, "SEARCH product.prod_name FOR 'DELETE999_Z' LIMIT 10")
            .await
            .unwrap();
        assert!(result.rows.is_empty());
    }

    #[tokio::test]
    async fn search_without_index_errors_clearly() {
        let e = executor();
        let err = run(&e, "SEARCH product.prod_name FOR \"abc\"")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("REBUILD TEXT INDEX"), "got: {err}");
    }
}

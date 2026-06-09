// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Cypher-like MATCH execution
//!
//! This module consumes the structured `byoridb_parser::ast::Pattern` carried
//! by `MatchPlan` and walks the stored graph, applying label and property
//! filters via [`PatternMatcher::matches_node`] / `matches_edge_data`.

mod match_executor;
mod pattern_matcher;

pub use match_executor::MatchExecutor;
pub use pattern_matcher::PatternMatcher;

// Re-exported for the EXPLAIN/PROFILE plan-tree builder so it can statically
// determine which tag index a MATCH start-node pattern would use, and detect
// the reverse single-edge optimisation (WHERE id(end)==X) it would apply.
pub(crate) use match_executor::{extract_id_eq_bindings, pick_index_plan};

#[cfg(test)]
mod tests {
    use super::match_executor::{pick_index_plan, MatchExecutor};
    use super::*;
    use crate::context::ExecutionContext;
    use byoridb_codec::{EdgeData, TagData, VertexCodec, VertexData};
    use byoridb_kvstore::store::MemoryKVStore;
    use byoridb_parser::ast::{self, Expression, Literal};
    use byoridb_storage::index::IndexDef;
    use byoridb_storage::key::IndexValue;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn make_matcher() -> PatternMatcher {
        let kvstore = Arc::new(MemoryKVStore::new());
        let ctx = Arc::new(ExecutionContext::new(kvstore).with_space("default".to_string()));
        PatternMatcher::new(ctx)
    }

    fn make_context() -> Arc<ExecutionContext> {
        let kvstore = Arc::new(MemoryKVStore::new());
        Arc::new(
            ExecutionContext::new(kvstore)
                .with_space("default".to_string())
                .with_space_id(1),
        )
    }

    fn vertex_blob(tags: &[(&str, &[(&str, serde_json::Value)])]) -> Vec<u8> {
        let tag_data: Vec<TagData> = tags
            .iter()
            .map(|(name, props)| {
                let properties = props
                    .iter()
                    .map(|(k, v)| {
                        let val = match v {
                            serde_json::Value::Bool(b) => byoridb_common::Value::Bool(*b),
                            serde_json::Value::Number(n) => {
                                if let Some(i) = n.as_i64() {
                                    byoridb_common::Value::Int(i)
                                } else {
                                    byoridb_common::Value::Float(n.as_f64().unwrap_or(0.0))
                                }
                            }
                            serde_json::Value::String(s) => {
                                byoridb_common::Value::String(s.clone())
                            }
                            _ => byoridb_common::Value::Null(byoridb_common::NullType::Null),
                        };
                        (k.to_string(), val)
                    })
                    .collect();
                TagData {
                    name: name.to_string(),
                    properties,
                }
            })
            .collect();
        VertexCodec::encode_vertex(&VertexData {
            vid: 0,
            tags: tag_data,
        })
        .unwrap()
    }

    fn edge_data_with(props: &[(&str, byoridb_common::Value)]) -> EdgeData {
        let mut properties = HashMap::new();
        for (k, v) in props {
            properties.insert(k.to_string(), v.clone());
        }
        EdgeData {
            src_vid: 0,
            dst_vid: 0,
            edge_type: "follow".to_string(),
            ranking: 0,
            properties,
        }
    }

    fn node_pattern_with(labels: Vec<&str>, props: Vec<(&str, Expression)>) -> ast::NodePattern {
        ast::NodePattern {
            variable: Some("n".to_string()),
            labels: labels.into_iter().map(|s| s.to_string()).collect(),
            props: props.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        }
    }

    fn edge_pattern_with(props: Vec<(&str, Expression)>) -> ast::EdgePattern {
        ast::EdgePattern {
            variable: Some("e".to_string()),
            edge_types: vec![],
            direction: ast::EdgeDirection::Outgoing,
            props: props.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
            range: None,
        }
    }

    #[tokio::test]
    async fn find_node_candidates_uses_tag_index_for_label_property_pattern() {
        let ctx = make_context();
        let index_manager = ctx.index_manager.as_ref().unwrap();
        let index_id = index_manager
            .create_tag_index(
                1,
                "person_name_idx".to_string(),
                10,
                "person".to_string(),
                vec!["name".to_string()],
                vec![0],
            )
            .await
            .unwrap();

        let alice_blob = vertex_blob(&[("person", &[("name", serde_json::json!("Alice"))])]);
        ctx.kvstore
            .put(b"default:vertex:100", &alice_blob)
            .await
            .unwrap();

        let unindexed_alice_blob =
            vertex_blob(&[("person", &[("name", serde_json::json!("Alice"))])]);
        ctx.kvstore
            .put(b"default:vertex:101", &unindexed_alice_blob)
            .await
            .unwrap();

        index_manager
            .insert_tag_index(1, index_id, &[IndexValue::String("Alice".to_string())], 100)
            .await
            .unwrap();

        let executor = MatchExecutor::new(ctx.clone());
        let matcher = PatternMatcher::new(ctx);
        let pattern = node_pattern_with(
            vec!["person"],
            vec![(
                "name",
                Expression::Literal(Literal::String("Alice".to_string())),
            )],
        );

        let candidates = executor
            .find_node_candidates("default", &pattern, &matcher)
            .await
            .unwrap();

        assert_eq!(candidates, vec![100]);
    }

    #[test]
    fn matches_node_accepts_empty_pattern() {
        let matcher = make_matcher();
        let blob = vertex_blob(&[("person", &[])]);
        let pattern = node_pattern_with(vec![], vec![]);
        assert!(matcher.matches_node(&blob, &pattern).unwrap());
    }

    #[test]
    fn matches_node_rejects_on_label_mismatch() {
        let matcher = make_matcher();
        let blob = vertex_blob(&[("person", &[])]);
        let pattern = node_pattern_with(vec!["team"], vec![]);
        assert!(!matcher.matches_node(&blob, &pattern).unwrap());
    }

    #[test]
    fn matches_node_accepts_on_label_match() {
        let matcher = make_matcher();
        let blob = vertex_blob(&[("person", &[])]);
        let pattern = node_pattern_with(vec!["person"], vec![]);
        assert!(matcher.matches_node(&blob, &pattern).unwrap());
    }

    #[test]
    fn matches_node_filters_by_property() {
        let matcher = make_matcher();
        let blob = vertex_blob(&[(
            "person",
            &[
                ("name", serde_json::json!("Alice")),
                ("age", serde_json::json!(30)),
            ],
        )]);
        let pattern_match = node_pattern_with(
            vec!["person"],
            vec![(
                "name",
                Expression::Literal(Literal::String("Alice".to_string())),
            )],
        );
        assert!(matcher.matches_node(&blob, &pattern_match).unwrap());

        let pattern_mismatch = node_pattern_with(
            vec!["person"],
            vec![(
                "name",
                Expression::Literal(Literal::String("Bob".to_string())),
            )],
        );
        assert!(!matcher.matches_node(&blob, &pattern_mismatch).unwrap());
    }

    #[test]
    fn matches_node_requires_all_properties() {
        let matcher = make_matcher();
        let blob = vertex_blob(&[(
            "person",
            &[
                ("name", serde_json::json!("Alice")),
                ("age", serde_json::json!(30)),
            ],
        )]);
        // age matches but name is missing
        let pattern = node_pattern_with(
            vec![],
            vec![
                ("name", Expression::Literal(Literal::String("Bob".into()))),
                ("age", Expression::Literal(Literal::Int(30))),
            ],
        );
        assert!(!matcher.matches_node(&blob, &pattern).unwrap());
    }

    #[test]
    fn matches_node_ignores_non_json_blobs() {
        let matcher = make_matcher();
        let blob = b"not json".to_vec();
        let pattern = node_pattern_with(vec!["person"], vec![]);
        assert!(!matcher.matches_node(&blob, &pattern).unwrap());
    }

    #[test]
    fn matches_edge_accepts_empty_pattern() {
        let matcher = make_matcher();
        let edge = edge_data_with(&[]);
        let pattern = edge_pattern_with(vec![]);
        assert!(matcher.matches_edge_data(&edge, &pattern).unwrap());
    }

    #[test]
    fn matches_edge_filters_by_property() {
        let matcher = make_matcher();
        let edge = edge_data_with(&[("since", byoridb_common::Value::Int(2020))]);

        let pattern_match =
            edge_pattern_with(vec![("since", Expression::Literal(Literal::Int(2020)))]);
        assert!(matcher.matches_edge_data(&edge, &pattern_match).unwrap());

        let pattern_mismatch =
            edge_pattern_with(vec![("since", Expression::Literal(Literal::Int(2021)))]);
        assert!(!matcher.matches_edge_data(&edge, &pattern_mismatch).unwrap());
    }

    // ---- pick_index_plan ----

    fn make_index(name: &str, schema: &str, fields: &[&str]) -> IndexDef {
        IndexDef {
            id: 0,
            space_id: 1,
            index_name: name.to_string(),
            index_type: byoridb_storage::index::IndexType::Tag,
            schema_id: 0,
            schema_name: schema.to_string(),
            fields: fields.iter().map(|s| s.to_string()).collect(),
            field_indices: vec![0; fields.len()],
        }
    }

    #[test]
    fn pick_index_plan_returns_none_when_no_indexes() {
        let plan = pick_index_plan(&[], &[("name".to_string(), byoridb_common::Value::Int(1))]);
        assert!(plan.is_none());
    }

    #[test]
    fn pick_index_plan_prefers_full_cover_over_single_field() {
        let full = make_index("person_name_age", "person", &["name", "age"]);
        let single = make_index("person_name", "person", &["name"]);
        let indexes: Vec<&IndexDef> = vec![&full, &single];

        let props = vec![
            (
                "name".to_string(),
                byoridb_common::Value::String("A".into()),
            ),
            ("age".to_string(), byoridb_common::Value::Int(30)),
        ];
        let (chosen, values) = pick_index_plan(&indexes, &props).unwrap();
        assert_eq!(chosen.index_name, "person_name_age");
        assert_eq!(values.len(), 2);
    }

    #[test]
    fn pick_index_plan_uses_prefix_when_no_full_cover() {
        // multi-field index, but pattern only has leading prefix
        let idx = make_index("person_name_age_city", "person", &["name", "age", "city"]);
        let indexes: Vec<&IndexDef> = vec![&idx];

        let props = vec![
            (
                "name".to_string(),
                byoridb_common::Value::String("A".into()),
            ),
            ("age".to_string(), byoridb_common::Value::Int(30)),
        ];
        let (chosen, values) = pick_index_plan(&indexes, &props).unwrap();
        assert_eq!(chosen.index_name, "person_name_age_city");
        // Prefix of length 2 (name, age)
        assert_eq!(values.len(), 2);
    }

    #[test]
    fn pick_index_plan_skips_index_with_no_leading_field_match() {
        // Pattern has 'city' but index leads with 'name', so prefix_len=0 → skip
        let idx = make_index("person_name_age", "person", &["name", "age"]);
        let indexes: Vec<&IndexDef> = vec![&idx];

        let props = vec![(
            "city".to_string(),
            byoridb_common::Value::String("Seoul".into()),
        )];
        assert!(pick_index_plan(&indexes, &props).is_none());
    }

    #[test]
    fn pick_index_plan_picks_longest_prefix_on_tie() {
        let short = make_index("idx_short", "person", &["name", "city"]);
        let long = make_index("idx_long", "person", &["name", "age", "city"]);
        let indexes: Vec<&IndexDef> = vec![&short, &long];

        // Pattern has name + age — long matches prefix 2, short matches prefix 1.
        let props = vec![
            (
                "name".to_string(),
                byoridb_common::Value::String("A".into()),
            ),
            ("age".to_string(), byoridb_common::Value::Int(30)),
        ];
        let (chosen, values) = pick_index_plan(&indexes, &props).unwrap();
        assert_eq!(chosen.index_name, "idx_long");
        assert_eq!(values.len(), 2);
    }
}

#[cfg(test)]
mod h6_multipattern_tests {
    use super::match_executor::MatchExecutor;
    use crate::context::ExecutionContext;
    use crate::plan::{ExecutionPlan, ExecutionPlanBuilder};
    use byoridb_codec::{EdgeData, TagData, VertexCodec, VertexData};
    use byoridb_kvstore::store::MemoryKVStore;
    use std::sync::Arc;

    fn vblob(tag: &str, name: &str) -> Vec<u8> {
        VertexCodec::encode_vertex(&VertexData {
            vid: 0,
            tags: vec![TagData {
                name: tag.to_string(),
                properties: [(
                    "name".to_string(),
                    byoridb_common::Value::String(name.to_string()),
                )]
                .into_iter()
                .collect(),
            }],
        })
        .unwrap()
    }

    fn eblob(src: i64, dst: i64, etype: &str) -> Vec<u8> {
        VertexCodec::encode_edge(&EdgeData {
            src_vid: src,
            dst_vid: dst,
            edge_type: etype.to_string(),
            ranking: 0,
            properties: Default::default(),
        })
        .unwrap()
    }

    async fn seed(ctx: &Arc<ExecutionContext>) {
        // p1, p2 (product) → c (category); p1 → t1, p2 → t2 (tag)
        ctx.kvstore
            .put(b"default:vertex:1", &vblob("product", "P1"))
            .await
            .unwrap();
        ctx.kvstore
            .put(b"default:vertex:2", &vblob("product", "P2"))
            .await
            .unwrap();
        ctx.kvstore
            .put(b"default:vertex:100", &vblob("category", "C"))
            .await
            .unwrap();
        ctx.kvstore
            .put(b"default:vertex:200", &vblob("itemtag", "T1"))
            .await
            .unwrap();
        ctx.kvstore
            .put(b"default:vertex:201", &vblob("itemtag", "T2"))
            .await
            .unwrap();
        ctx.kvstore
            .put(
                b"default:edge:1:belongs_to:100:0",
                &eblob(1, 100, "belongs_to"),
            )
            .await
            .unwrap();
        ctx.kvstore
            .put(
                b"default:edge:2:belongs_to:100:0",
                &eblob(2, 100, "belongs_to"),
            )
            .await
            .unwrap();
        ctx.kvstore
            .put(b"default:edge:1:has_tag:200:0", &eblob(1, 200, "has_tag"))
            .await
            .unwrap();
        ctx.kvstore
            .put(b"default:edge:2:has_tag:201:0", &eblob(2, 201, "has_tag"))
            .await
            .unwrap();
        // Reverse-edge index entries (key: {space}:in-edge:{dst}:{type}:{src}:{rank}),
        // mirroring the production INSERT EDGE write so reverse traversal works.
        ctx.kvstore
            .put(
                b"default:in-edge:100:belongs_to:1:0",
                &eblob(1, 100, "belongs_to"),
            )
            .await
            .unwrap();
        ctx.kvstore
            .put(
                b"default:in-edge:100:belongs_to:2:0",
                &eblob(2, 100, "belongs_to"),
            )
            .await
            .unwrap();
        ctx.kvstore
            .put(
                b"default:in-edge:200:has_tag:1:0",
                &eblob(1, 200, "has_tag"),
            )
            .await
            .unwrap();
        ctx.kvstore
            .put(
                b"default:in-edge:201:has_tag:2:0",
                &eblob(2, 201, "has_tag"),
            )
            .await
            .unwrap();
    }

    fn match_plan(q: &str) -> crate::plan::MatchPlan {
        let stmt = byoridb_parser::parse(q).unwrap();
        match ExecutionPlanBuilder::build(stmt).unwrap() {
            ExecutionPlan::Match(m) => m,
            other => panic!(
                "expected Match plan, got {:?}",
                std::any::type_name_of_val(&other)
            ),
        }
    }

    #[tokio::test]
    async fn multipattern_inner_join_binds_both_patterns() {
        let kv = Arc::new(MemoryKVStore::new());
        let ctx = Arc::new(
            ExecutionContext::new(kv)
                .with_space("default".to_string())
                .with_space_id(1),
        );
        seed(&ctx).await;
        let exec = MatchExecutor::new(ctx.clone());

        // Both p1 and p2 belong to c(100) and each has a tag → 2 rows.
        let plan = match_plan(
            "MATCH (p:product)-[:belongs_to]->(c:category), (p)-[:has_tag]->(t:itemtag) \
             WHERE id(c)==100 RETURN p.product.name AS name, t.itemtag.name AS tname",
        );
        let res = exec.execute_match(plan).await.unwrap();
        assert_eq!(res.rows.len(), 2, "both products joined with their tags");
    }

    #[tokio::test]
    async fn multipattern_respects_limit() {
        let kv = Arc::new(MemoryKVStore::new());
        let ctx = Arc::new(
            ExecutionContext::new(kv)
                .with_space("default".to_string())
                .with_space_id(1),
        );
        seed(&ctx).await;
        let exec = MatchExecutor::new(ctx.clone());

        // H-6: LIMIT 1 must cap output (previously LIMIT was dropped entirely).
        let plan = match_plan(
            "MATCH (p:product)-[:belongs_to]->(c:category), (p)-[:has_tag]->(t:itemtag) \
             WHERE id(c)==100 RETURN p.product.name AS name LIMIT 1",
        );
        let res = exec.execute_match(plan).await.unwrap();
        assert_eq!(res.rows.len(), 1, "LIMIT 1 must cap the joined result");
    }
}

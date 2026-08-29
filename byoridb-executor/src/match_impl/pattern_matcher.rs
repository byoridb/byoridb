// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Cypher-like MATCH execution
//!
//! This module consumes the structured `byoridb_parser::ast::Pattern` carried
//! by `MatchPlan` and walks the stored graph, applying label and property
//! filters via [`PatternMatcher::matches_node`] / `matches_edge_data`.

use crate::context::ExecutionContext;
use crate::error::{ExecutionError, Result};
use byoridb_codec::{EdgeData, VertexCodec};
use byoridb_parser::ast::{EdgePattern, NodePattern};
use std::sync::Arc;

use super::match_executor::{byoridb_value_equals, expression_as_value};

pub struct PatternMatcher {
    #[allow(dead_code)]
    ctx: Arc<ExecutionContext>,
}

impl PatternMatcher {
    pub fn new(ctx: Arc<ExecutionContext>) -> Self {
        Self { ctx }
    }

    /// Check if a vertex blob matches a node pattern.
    ///
    /// Decodes the vertex using `VertexCodec` (supports both proto and JSON
    /// on-disk formats) and checks label and property filters against the
    /// decoded `VertexData`. Returns true when there are no filters.
    pub(crate) fn matches_node(&self, vertex_blob: &[u8], pattern: &NodePattern) -> Result<bool> {
        if pattern.labels.is_empty() && pattern.props.is_empty() {
            return Ok(true);
        }

        let vertex = match VertexCodec::decode_vertex(vertex_blob) {
            Ok(v) => v,
            Err(_) => return Ok(false),
        };

        // Label filter. `(n:A:B)` requires every listed tag; `(n:A|B)` requires at
        // least one (#124). Before this the matcher always accepted "at least
        // one" while candidate selection used only the first label, so a
        // multi-label pattern silently meant its first label — and changing the
        // order changed the answer.
        if !pattern.labels.is_empty() {
            let carries = |label: &String| {
                vertex
                    .tags
                    .iter()
                    .any(|tag| label.eq_ignore_ascii_case(&tag.name))
            };
            let matched = match pattern.label_match {
                byoridb_parser::ast::LabelMatch::All => pattern.labels.iter().all(carries),
                byoridb_parser::ast::LabelMatch::Any => pattern.labels.iter().any(carries),
            };
            if !matched {
                return Ok(false);
            }
        }

        // Property filter: every required key must be present on some tag
        // and equal the pattern's literal value.
        if !pattern.props.is_empty() {
            for (key, expected) in &pattern.props {
                let expected_val = expression_as_value(expected).ok_or_else(|| {
                    ExecutionError::InvalidOperation(format!(
                        "Unsupported literal in pattern property '{}'",
                        key
                    ))
                })?;

                let found = vertex.tags.iter().any(|tag| {
                    tag.properties
                        .get(key)
                        .map(|v| byoridb_value_equals(v, &expected_val))
                        .unwrap_or(false)
                });
                if !found {
                    return Ok(false);
                }
            }
        }

        Ok(true)
    }

    /// Check if a decoded edge matches an edge pattern's property filter.
    ///
    /// Edge-type filtering is handled by the walker through
    /// [`algo::get_neighbors`], so this only enforces the pattern's
    /// `props` map.
    pub(crate) fn matches_edge_data(&self, edge: &EdgeData, pattern: &EdgePattern) -> Result<bool> {
        if pattern.props.is_empty() {
            return Ok(true);
        }

        for (key, expected) in &pattern.props {
            let expected_val = expression_as_value(expected).ok_or_else(|| {
                ExecutionError::InvalidOperation(format!(
                    "Unsupported literal in edge property '{}'",
                    key
                ))
            })?;

            let found = edge
                .properties
                .get(key)
                .map(|v| byoridb_value_equals(v, &expected_val))
                .unwrap_or(false);
            if !found {
                return Ok(false);
            }
        }

        Ok(true)
    }
}

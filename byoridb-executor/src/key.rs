// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Schema key utilities for consistent key construction

/// Schema key builder for metadata and data storage
///
/// Key formats:
///
/// Schema keys (metadata):
/// - Space: `space:{name}`
/// - Tag: `space:{space}:tag:{name}`
/// - Edge: `space:{space}:edge:{name}`
/// - Tag index: `space:{space}:tag_index:{name}`
/// - Edge index: `space:{space}:edge_index:{name}`
///
/// Data keys (vertex/edge data):
/// - Vertex: `{space}:vertex:{vid}`
/// - Edge data: `{space}:edge:{src}:{edge_type_id}:{ranking}`
///
/// System keys (user management):
/// - User: `__user_{username}`
pub struct SchemaKey;

/// User key prefix for user management storage
/// Format: `__user_{username}`
pub const USER_KEY_PREFIX: &str = "__user_";

impl SchemaKey {
    // ===== Space keys =====

    /// Create a space key: `space:{name}`
    pub fn space(name: &str) -> Vec<u8> {
        format!("space:{}", name).into_bytes()
    }

    /// Create a space prefix for scanning all spaces: `space:`
    pub fn space_prefix() -> Vec<u8> {
        b"space:".to_vec()
    }

    // ===== Tag keys =====

    /// Create a tag key: `space:{space}:tag:{name}`
    pub fn tag(space: &str, name: &str) -> Vec<u8> {
        format!("space:{}:tag:{}", space, name).into_bytes()
    }

    /// Create a tag prefix for scanning all tags in a space: `space:{space}:tag:`
    pub fn tag_prefix(space: &str) -> Vec<u8> {
        format!("space:{}:tag:", space).into_bytes()
    }

    // ===== Edge keys =====

    /// Create an edge type key: `space:{space}:edge:{name}`
    pub fn edge(space: &str, name: &str) -> Vec<u8> {
        format!("space:{}:edge:{}", space, name).into_bytes()
    }

    /// Create an edge prefix for scanning all edges in a space: `space:{space}:edge:`
    pub fn edge_prefix(space: &str) -> Vec<u8> {
        format!("space:{}:edge:", space).into_bytes()
    }

    // ===== Vertex data keys =====

    /// Create a vertex data key: `{space}:vertex:{vid}`
    pub fn vertex(space: &str, vid: i64) -> Vec<u8> {
        format!("{}:vertex:{}", space, vid).into_bytes()
    }

    /// Create a vertex prefix for scanning all vertices in a space: `{space}:vertex:`
    pub fn vertex_prefix(space: &str) -> Vec<u8> {
        format!("{}:vertex:", space).into_bytes()
    }

    // ===== Edge data keys =====

    /// Create an edge data key: `{space}:edge:{src}:{edge_type}:{dst}:{ranking}`
    pub fn edge_data(space: &str, src: i64, edge_type: &str, dst: i64, ranking: i64) -> Vec<u8> {
        format!("{}:edge:{}:{}:{}:{}", space, src, edge_type, dst, ranking).into_bytes()
    }

    /// Create an edge data prefix for all edges from a vertex: `{space}:edge:{src}:`
    pub fn edge_data_src_prefix(space: &str, src: i64) -> Vec<u8> {
        format!("{}:edge:{}:", space, src).into_bytes()
    }

    // ===== Index keys =====

    /// Create a tag index key: `space:{space}:tag_index:{name}`
    pub fn tag_index(space: &str, name: &str) -> Vec<u8> {
        format!("space:{}:tag_index:{}", space, name).into_bytes()
    }

    /// Create an edge index key: `space:{space}:edge_index:{name}`
    pub fn edge_index(space: &str, name: &str) -> Vec<u8> {
        format!("space:{}:edge_index:{}", space, name).into_bytes()
    }

    /// Counter key for auto-incrementing space IDs: `__meta:next_space_id`
    pub fn next_space_id_key() -> Vec<u8> {
        b"__meta:next_space_id".to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_space_key() {
        assert_eq!(SchemaKey::space("my_space"), b"space:my_space".to_vec());
    }

    #[test]
    fn test_tag_key() {
        assert_eq!(
            SchemaKey::tag("my_space", "person"),
            b"space:my_space:tag:person".to_vec()
        );
    }

    #[test]
    fn test_edge_key() {
        assert_eq!(
            SchemaKey::edge("my_space", "knows"),
            b"space:my_space:edge:knows".to_vec()
        );
    }

    #[test]
    fn test_vertex_key() {
        assert_eq!(
            SchemaKey::vertex("my_space", 123),
            b"my_space:vertex:123".to_vec()
        );
    }

    #[test]
    fn test_edge_data_key() {
        assert_eq!(
            SchemaKey::edge_data("my_space", 1, "knows", 2, 0),
            b"my_space:edge:1:knows:2:0".to_vec()
        );
    }

    #[test]
    fn test_vertex_prefix() {
        assert_eq!(
            SchemaKey::vertex_prefix("my_space"),
            b"my_space:vertex:".to_vec()
        );
    }

    #[test]
    fn test_edge_data_src_prefix() {
        assert_eq!(
            SchemaKey::edge_data_src_prefix("my_space", 1),
            b"my_space:edge:1:".to_vec()
        );
    }

    #[test]
    fn test_prefixes() {
        assert_eq!(SchemaKey::space_prefix(), b"space:".to_vec());
        assert_eq!(
            SchemaKey::tag_prefix("my_space"),
            b"space:my_space:tag:".to_vec()
        );
        assert_eq!(
            SchemaKey::edge_prefix("my_space"),
            b"space:my_space:edge:".to_vec()
        );
    }
}

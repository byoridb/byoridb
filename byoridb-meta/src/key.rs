// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Meta key utilities for consistent binary key encoding
//!
//! All keys use big-endian byte encoding for consistent sorting in RocksDB.

/// Meta key builder for metadata storage
///
/// Key formats (binary):
/// - Space: `space_<space_id:4>`
/// - Tag: `tag_<space_id:4>_<tag_id:4>`
/// - Edge: `edge_<space_id:4>_<edge_id:4>`
/// - Tag version: `tag_<space_id:4>_<tag_id:4>_v<version:4>`
/// - Edge version: `edge_<space_id:4>_<edge_id:4>_v<version:4>`
/// - Tag index: `tag_idx_<space_id:4>_<index_id:4>`
/// - Edge index: `edge_idx_<space_id:4>_<index_id:4>`
/// - Ring: `ring_<space_id:4>`
pub struct MetaKey;

impl MetaKey {
    // ===== Space keys =====

    /// Create a space key: `space_<space_id>`
    pub fn space(space_id: u32) -> Vec<u8> {
        let mut key = b"space_".to_vec();
        key.extend_from_slice(&space_id.to_be_bytes());
        key
    }

    /// Create a space prefix for scanning all spaces: `space_`
    pub fn space_prefix() -> Vec<u8> {
        b"space_".to_vec()
    }

    // ===== Tag keys =====

    /// Create a tag key: `tag_<space_id>_<tag_id>`
    pub fn tag(space_id: u32, tag_id: u32) -> Vec<u8> {
        let mut key = b"tag_".to_vec();
        key.extend_from_slice(&space_id.to_be_bytes());
        key.push(b'_');
        key.extend_from_slice(&tag_id.to_be_bytes());
        key
    }

    /// Create a tag version key: `tag_<space_id>_<tag_id>_v<version>`
    pub fn tag_version(space_id: u32, tag_id: u32, version: i32) -> Vec<u8> {
        let mut key = b"tag_".to_vec();
        key.extend_from_slice(&space_id.to_be_bytes());
        key.push(b'_');
        key.extend_from_slice(&tag_id.to_be_bytes());
        key.extend_from_slice(b"_v");
        key.extend_from_slice(&version.to_be_bytes());
        key
    }

    /// Create a tag prefix for scanning all tags in a space: `tag_<space_id>_`
    pub fn tag_prefix(space_id: u32) -> Vec<u8> {
        let mut key = b"tag_".to_vec();
        key.extend_from_slice(&space_id.to_be_bytes());
        key.push(b'_');
        key
    }

    // ===== Edge keys =====

    /// Create an edge key: `edge_<space_id>_<edge_id>`
    pub fn edge(space_id: u32, edge_id: u32) -> Vec<u8> {
        let mut key = b"edge_".to_vec();
        key.extend_from_slice(&space_id.to_be_bytes());
        key.push(b'_');
        key.extend_from_slice(&edge_id.to_be_bytes());
        key
    }

    /// Create an edge version key: `edge_<space_id>_<edge_id>_v<version>`
    pub fn edge_version(space_id: u32, edge_id: u32, version: i32) -> Vec<u8> {
        let mut key = b"edge_".to_vec();
        key.extend_from_slice(&space_id.to_be_bytes());
        key.push(b'_');
        key.extend_from_slice(&edge_id.to_be_bytes());
        key.extend_from_slice(b"_v");
        key.extend_from_slice(&version.to_be_bytes());
        key
    }

    /// Create an edge prefix for scanning all edges in a space: `edge_<space_id>_`
    pub fn edge_prefix(space_id: u32) -> Vec<u8> {
        let mut key = b"edge_".to_vec();
        key.extend_from_slice(&space_id.to_be_bytes());
        key.push(b'_');
        key
    }

    // ===== Index keys =====

    /// Create a tag index key: `tag_idx_<space_id>_<index_id>`
    pub fn tag_index(space_id: u32, index_id: u32) -> Vec<u8> {
        let mut key = b"tag_idx_".to_vec();
        key.extend_from_slice(&space_id.to_be_bytes());
        key.push(b'_');
        key.extend_from_slice(&index_id.to_be_bytes());
        key
    }

    /// Create an edge index key: `edge_idx_<space_id>_<index_id>`
    pub fn edge_index(space_id: u32, index_id: u32) -> Vec<u8> {
        let mut key = b"edge_idx_".to_vec();
        key.extend_from_slice(&space_id.to_be_bytes());
        key.push(b'_');
        key.extend_from_slice(&index_id.to_be_bytes());
        key
    }

    // ===== Ring keys =====

    /// Create a ring key for consistent hash ring: `ring_<space_id>`
    pub fn ring(space_id: u32) -> Vec<u8> {
        let mut key = b"ring_".to_vec();
        key.extend_from_slice(&space_id.to_be_bytes());
        key
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_space_key() {
        let key = MetaKey::space(1);
        assert!(key.starts_with(b"space_"));
        assert_eq!(key.len(), 10); // 6 + 4 bytes
    }

    #[test]
    fn test_tag_key() {
        let key = MetaKey::tag(1, 2);
        assert!(key.starts_with(b"tag_"));
        assert_eq!(key.len(), 13); // 4 + 4 + 1 + 4 bytes
    }

    #[test]
    fn test_tag_version_key() {
        let key = MetaKey::tag_version(1, 2, 3);
        assert!(key.starts_with(b"tag_"));
        assert!(key.windows(2).any(|w| w == b"_v"));
    }

    #[test]
    fn test_edge_key() {
        let key = MetaKey::edge(1, 2);
        assert!(key.starts_with(b"edge_"));
        assert_eq!(key.len(), 14); // 5 + 4 + 1 + 4 bytes
    }

    #[test]
    fn test_edge_version_key() {
        let key = MetaKey::edge_version(1, 2, 3);
        assert!(key.starts_with(b"edge_"));
        assert!(key.windows(2).any(|w| w == b"_v"));
    }

    #[test]
    fn test_index_keys() {
        let tag_idx = MetaKey::tag_index(1, 10);
        assert!(tag_idx.starts_with(b"tag_idx_"));

        let edge_idx = MetaKey::edge_index(1, 10);
        assert!(edge_idx.starts_with(b"edge_idx_"));
    }

    #[test]
    fn test_ring_key() {
        let key = MetaKey::ring(1);
        assert!(key.starts_with(b"ring_"));
        assert_eq!(key.len(), 9); // 5 + 4 bytes
    }

    #[test]
    fn test_prefixes() {
        assert_eq!(MetaKey::space_prefix(), b"space_".to_vec());
        assert!(MetaKey::tag_prefix(1).starts_with(b"tag_"));
        assert!(MetaKey::edge_prefix(1).starts_with(b"edge_"));
    }
}

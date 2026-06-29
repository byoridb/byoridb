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
/// - Reverse-edge (in-edge) index: `{space}:in-edge:{dst}:{edge_type}:{src}:{ranking}`
///   (denormalized edge value; enables O(in-degree) reverse traversal)
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

    // ===== Class keys (O-3 ontology TBox) =====

    /// Create a class metadata key: `space:{space}:class:{name}`.
    ///
    /// A class is a tag superset: the field schema lives under the normal
    /// tag key (so INSERT/MATCH/index machinery applies unchanged) and this
    /// key only carries the hierarchy metadata
    /// (`{"name": ..., "superclasses": [...]}`).
    pub fn class(space: &str, name: &str) -> Vec<u8> {
        format!("space:{}:class:{}", space, name).into_bytes()
    }

    /// Prefix for scanning all classes in a space: `space:{space}:class:`
    pub fn class_prefix(space: &str) -> Vec<u8> {
        format!("space:{}:class:", space).into_bytes()
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

    // ===== Reverse-edge (in-edge) index keys =====

    /// Create a reverse-edge data key: `{space}:in-edge:{dst}:{edge_type}:{src}:{ranking}`.
    ///
    /// The value stored under this key is the same denormalized edge payload as
    /// the forward `edge_data` key, so reverse traversal needs no second lookup.
    /// `edge_type` lands on segment index 3 — identical to the forward key — so
    /// the shared `edge_type_from_key` filter works for both directions.
    pub fn in_edge_data(space: &str, dst: i64, edge_type: &str, src: i64, ranking: i64) -> Vec<u8> {
        format!(
            "{}:in-edge:{}:{}:{}:{}",
            space, dst, edge_type, src, ranking
        )
        .into_bytes()
    }

    /// Create an in-edge prefix for all edges pointing into a vertex: `{space}:in-edge:{dst}:`
    pub fn in_edge_data_dst_prefix(space: &str, dst: i64) -> Vec<u8> {
        format!("{}:in-edge:{}:", space, dst).into_bytes()
    }

    // ===== Inferred vertex type membership (PLAN.md O-5 domain/range) =====

    /// Inferred class membership for a vertex: `{space}:vtype:{vid}:{class}` →
    /// empty. Written by domain/range materialization; consulted by
    /// `ontology::vertex_class_set` so `is_a(...)` sees inferred types.
    pub fn vtype(space: &str, vid: i64, class: &str) -> Vec<u8> {
        format!("{}:vtype:{}:{}", space, vid, class).into_bytes()
    }

    /// Prefix for all inferred classes of a vertex: `{space}:vtype:{vid}:`
    pub fn vtype_prefix(space: &str, vid: i64) -> Vec<u8> {
        format!("{}:vtype:{}:", space, vid).into_bytes()
    }

    /// Extract the trailing class name from a `vtype` key.
    pub fn vtype_class_from_key(key: &[u8]) -> Option<String> {
        let s = std::str::from_utf8(key).ok()?;
        s.rsplit(':').next().map(|c| c.to_string())
    }

    // ===== Tag-vid secondary index (label-only MATCH acceleration) =====

    /// Tag-vid index entry: `{space}:tagvid:{tag}:{vid}` → empty. Written by
    /// INSERT VERTEX so label-only MATCH can prefix-scan by tag instead of
    /// scanning every vertex. Note `vid` is the trailing segment, so a single
    /// vertex's tagvid entries cannot be gathered by one prefix — reconstruct
    /// them from the vertex blob's tags (see O-8 merge).
    pub fn tagvid(space: &str, tag: &str, vid: i64) -> Vec<u8> {
        format!("{}:tagvid:{}:{}", space, tag, vid).into_bytes()
    }

    // ===== owl:sameAs canonical-representative map (PLAN.md O-8) =====

    /// Union-find representative pointer for a vertex: `{space}:sameas:{vid}` →
    /// the canonical (min-id) vertex of its owl:sameAs equivalence class. Absent
    /// for vertices that have never been merged (they are their own representative).
    /// Consulted by `ontology::representative_of` to normalize GO/FETCH/MATCH vids.
    pub fn sameas(space: &str, vid: i64) -> Vec<u8> {
        format!("{}:sameas:{}", space, vid).into_bytes()
    }

    /// Reverse membership entry: `{space}:sameas-members:{rep}:{member}` → empty.
    /// Lets a representative enumerate the non-representative vids collapsed into
    /// it (used by DELETE guards and introspection). Note the `sameas-members:`
    /// infix keeps this keyspace disjoint from the `sameas:` pointer prefix.
    pub fn sameas_member(space: &str, rep: i64, member: i64) -> Vec<u8> {
        format!("{}:sameas-members:{}:{}", space, rep, member).into_bytes()
    }

    /// Prefix for all members collapsed into a representative:
    /// `{space}:sameas-members:{rep}:`
    pub fn sameas_members_prefix(space: &str, rep: i64) -> Vec<u8> {
        format!("{}:sameas-members:{}:", space, rep).into_bytes()
    }

    /// Extract the trailing member vid from a `sameas_member` key. The member vid
    /// is always the final colon-delimited segment.
    pub fn sameas_member_from_key(key: &[u8]) -> Option<i64> {
        let s = std::str::from_utf8(key).ok()?;
        s.rsplit(':').next()?.parse::<i64>().ok()
    }

    // ===== Dense embedding vector store (PLAN.md R-2a) =====

    /// Dense embedding entry: `{space}:vec:{prop}:{vid}` → packed little-endian
    /// f32 bytes. Written by INSERT VERTEX alongside the vertex blob for any
    /// numeric-list property, so cosine KNN scans only the packed floats instead
    /// of decoding full vertices.
    pub fn vec_data(space: &str, prop: &str, vid: i64) -> Vec<u8> {
        format!("{}:vec:{}:{}", space, prop, vid).into_bytes()
    }

    /// Prefix for all embedding entries of one property: `{space}:vec:{prop}:`
    pub fn vec_data_prop_prefix(space: &str, prop: &str) -> Vec<u8> {
        format!("{}:vec:{}:", space, prop).into_bytes()
    }

    /// Extract the trailing vid from a `vec_data` key. Property names are nGQL
    /// identifiers (no `:`), so the vid is always the final colon-delimited segment.
    pub fn vec_data_vid_from_key(key: &[u8]) -> Option<i64> {
        let s = std::str::from_utf8(key).ok()?;
        s.rsplit(':').next()?.parse::<i64>().ok()
    }

    /// Persisted HNSW index blob for one embedding property (PLAN.md R-2b):
    /// `{space}:vecidx:{prop}` → bincode(HnswMap). Built from the dense store,
    /// loaded for ANN search, rebuilt when the dirty marker is present.
    pub fn vec_index(space: &str, prop: &str) -> Vec<u8> {
        format!("{}:vecidx:{}", space, prop).into_bytes()
    }

    /// Dirty marker for a vector index: `{space}:vecidx-dirty:{prop}`. Present
    /// (empty value) iff the persisted index is stale w.r.t. the dense store.
    /// Set by INSERT/UPDATE of a numeric-list property; cleared on rebuild.
    pub fn vec_index_dirty(space: &str, prop: &str) -> Vec<u8> {
        format!("{}:vecidx-dirty:{}", space, prop).into_bytes()
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

    // ===== Edge degree counters (precomputed GROUP BY COUNT) =====

    /// In-degree counter: `{space}:indeg:{etype}:{dst}` → i64-LE count of edges
    /// of `etype` pointing into `dst`. Maintained by INSERT/DELETE EDGE and the
    /// bulk loader so `MATCH (d)<-[:etype]-() RETURN d, COUNT(*)` reads counters
    /// instead of scanning every edge.
    pub fn indeg_counter(space: &str, etype: &str, dst: i64) -> Vec<u8> {
        format!("{}:indeg:{}:{}", space, etype, dst).into_bytes()
    }

    /// Out-degree counter: `{space}:outdeg:{etype}:{src}` → i64-LE count.
    pub fn outdeg_counter(space: &str, etype: &str, src: i64) -> Vec<u8> {
        format!("{}:outdeg:{}:{}", space, etype, src).into_bytes()
    }

    /// Encode a degree counter value (8-byte little-endian, matching the
    /// `encode_repr` convention).
    pub fn encode_count(n: i64) -> Vec<u8> {
        n.to_le_bytes().to_vec()
    }

    /// Decode a degree counter value written by [`encode_count`].
    pub fn decode_count(bytes: &[u8]) -> Option<i64> {
        bytes.try_into().ok().map(i64::from_le_bytes)
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
    fn test_in_edge_data_key() {
        assert_eq!(
            SchemaKey::in_edge_data("my_space", 2, "knows", 1, 0),
            b"my_space:in-edge:2:knows:1:0".to_vec()
        );
    }

    #[test]
    fn test_in_edge_data_dst_prefix() {
        assert_eq!(
            SchemaKey::in_edge_data_dst_prefix("my_space", 2),
            b"my_space:in-edge:2:".to_vec()
        );
    }

    #[test]
    fn test_in_edge_edge_type_segment_matches_forward() {
        // Both forward and reverse keys must place edge_type at split segment 3
        // so the shared edge_type_from_key filter works for either direction.
        let fwd = SchemaKey::edge_data("s", 1, "knows", 2, 0);
        let rev = SchemaKey::in_edge_data("s", 2, "knows", 1, 0);
        let seg = |k: &[u8]| {
            std::str::from_utf8(k)
                .unwrap()
                .split(':')
                .nth(3)
                .unwrap()
                .to_string()
        };
        assert_eq!(seg(&fwd), "knows");
        assert_eq!(seg(&rev), "knows");
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
    fn test_sameas_keys() {
        assert_eq!(SchemaKey::sameas("s", 5), b"s:sameas:5".to_vec());
        assert_eq!(
            SchemaKey::sameas_member("s", 1, 5),
            b"s:sameas-members:1:5".to_vec()
        );
        assert_eq!(
            SchemaKey::sameas_members_prefix("s", 1),
            b"s:sameas-members:1:".to_vec()
        );
        // The pointer prefix must not collide with the members keyspace.
        assert!(!SchemaKey::sameas_member("s", 1, 5).starts_with(b"s:sameas:"));
        assert_eq!(
            SchemaKey::sameas_member_from_key(b"s:sameas-members:1:42"),
            Some(42)
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

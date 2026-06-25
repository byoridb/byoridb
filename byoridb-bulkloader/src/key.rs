// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Key builders reproduced from `byoridb_executor::SchemaKey`.
//!
//! The loader writes directly into the redb keyspace, so every key it produces
//! MUST be byte-identical to what the INSERT path (`executor/dml.rs`) writes —
//! otherwise the engine's MATCH/GO/FETCH would not find the loaded data.
//!
//! `SchemaKey` lives in `byoridb-executor`, a heavy crate (parser + storage +
//! the whole execution engine). To keep the loader's runtime dependency light
//! we reproduce the (trivial, stable) `format!` builders here and pin them to
//! the originals with byte-equality tests against `byoridb-executor` as a
//! dev-dependency (see `tests/`).

/// `space:{name}` — space metadata (read-only for the loader).
pub fn space(name: &str) -> Vec<u8> {
    format!("space:{}", name).into_bytes()
}

/// `space:{space}:tag:{name}` — tag schema (read-only for the loader).
pub fn tag(space: &str, name: &str) -> Vec<u8> {
    format!("space:{}:tag:{}", space, name).into_bytes()
}

/// `space:{space}:edge:{name}` — edge schema (read-only for the loader).
pub fn edge(space: &str, name: &str) -> Vec<u8> {
    format!("space:{}:edge:{}", space, name).into_bytes()
}

/// `{space}:vertex:{vid}` — vertex blob body.
pub fn vertex(space: &str, vid: i64) -> Vec<u8> {
    format!("{}:vertex:{}", space, vid).into_bytes()
}

/// `{space}:vertex:` — prefix for counting/scanning all vertices.
pub fn vertex_prefix(space: &str) -> Vec<u8> {
    format!("{}:vertex:", space).into_bytes()
}

/// `{space}:tagvid:{tag}:{vid}` — label-only MATCH acceleration index.
pub fn tagvid(space: &str, tag: &str, vid: i64) -> Vec<u8> {
    format!("{}:tagvid:{}:{}", space, tag, vid).into_bytes()
}

/// `{space}:tagvid:` — prefix for counting all tag-vid entries.
pub fn tagvid_prefix(space: &str) -> Vec<u8> {
    format!("{}:tagvid:", space).into_bytes()
}

/// `{space}:edge:{src}:{edge_type}:{dst}:{ranking}` — forward edge.
pub fn edge_data(space: &str, src: i64, edge_type: &str, dst: i64, ranking: i64) -> Vec<u8> {
    format!("{}:edge:{}:{}:{}:{}", space, src, edge_type, dst, ranking).into_bytes()
}

/// `{space}:in-edge:{dst}:{edge_type}:{src}:{ranking}` — reverse edge index.
/// Value is the SAME denormalized payload as the forward key.
pub fn in_edge_data(space: &str, dst: i64, edge_type: &str, src: i64, ranking: i64) -> Vec<u8> {
    format!(
        "{}:in-edge:{}:{}:{}:{}",
        space, dst, edge_type, src, ranking
    )
    .into_bytes()
}

/// `{space}:edge:` — prefix for counting all forward edges.
pub fn edge_prefix(space: &str) -> Vec<u8> {
    format!("{}:edge:", space).into_bytes()
}

/// `{space}:in-edge:` — prefix for counting all reverse edges.
pub fn in_edge_prefix(space: &str) -> Vec<u8> {
    format!("{}:in-edge:", space).into_bytes()
}

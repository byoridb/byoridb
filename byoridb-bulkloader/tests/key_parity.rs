// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! The loader reproduces `SchemaKey`'s `format!` builders locally to keep its
//! runtime dependency light. These tests pin every reproduced key to the
//! engine's `SchemaKey` so a format drift fails the build instead of silently
//! writing keys the engine can't find.

use byoridb_bulkloader::key;
use byoridb_executor::SchemaKey;

#[test]
fn vertex_and_tagvid_keys_match_engine() {
    assert_eq!(
        key::vertex("nexprice", 42),
        SchemaKey::vertex("nexprice", 42)
    );
    assert_eq!(
        key::vertex_prefix("nexprice"),
        SchemaKey::vertex_prefix("nexprice")
    );
    assert_eq!(
        key::tagvid("nexprice", "sku", 42),
        SchemaKey::tagvid("nexprice", "sku", 42)
    );
}

#[test]
fn edge_keys_match_engine() {
    assert_eq!(
        key::edge_data("nexprice", 1, "same_as", 2, 0),
        SchemaKey::edge_data("nexprice", 1, "same_as", 2, 0)
    );
    assert_eq!(
        key::in_edge_data("nexprice", 2, "same_as", 1, 0),
        SchemaKey::in_edge_data("nexprice", 2, "same_as", 1, 0)
    );
}

#[test]
fn metadata_keys_match_engine() {
    assert_eq!(key::space("nexprice"), SchemaKey::space("nexprice"));
    assert_eq!(
        key::tag("nexprice", "sku"),
        SchemaKey::tag("nexprice", "sku")
    );
    assert_eq!(
        key::edge("nexprice", "same_as"),
        SchemaKey::edge("nexprice", "same_as")
    );
}

#[test]
fn count_prefixes_are_correct_for_negative_vids_too() {
    // Sanity: prefix is a literal cut, so a vertex key always starts with it.
    assert!(key::vertex("s", 7).starts_with(&key::vertex_prefix("s")));
    assert!(key::tagvid("s", "t", 7).starts_with(&key::tagvid_prefix("s")));
    assert!(key::edge_data("s", 1, "e", 2, 0).starts_with(&key::edge_prefix("s")));
    assert!(key::in_edge_data("s", 2, "e", 1, 0).starts_with(&key::in_edge_prefix("s")));
    // The forward-edge prefix must not catch reverse-edge keys.
    assert!(!key::in_edge_data("s", 2, "e", 1, 0).starts_with(&key::edge_prefix("s")));
}

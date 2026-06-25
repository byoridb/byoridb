// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! `byoridb-bulkloader` — offline bulk import.
//!
//! Writes vertices/edges directly into the redb store, bypassing the
//! nGQL/HTTP/session path. Built for datasets too large for row-by-row INSERT
//! (tens to hundreds of millions of elements). Assigns sequential INT64 vids so
//! the redb B-tree stays append-friendly (avoids the random-VID write
//! amplification measured in commit 39f726e).
//!
//! ## Prerequisites
//! - The server must be **stopped** (redb is single-writer per process).
//! - The space, tags, and edge types must already exist — run
//!   `CREATE SPACE / TAG / EDGE` via the server first, then stop it.
//!
//! ## Example
//! ```text
//! byoridb-bulkloader --db /data --space nexprice \
//!   --node sku=sku.csv --node product=product.csv.gz \
//!   --edge same_as=same_as.csv --edge has_brand=has_brand.csv \
//!   --id-column id --src-column src --dst-column dst \
//!   --batch-size 100000 --durability relaxed --verify
//! ```

use anyhow::{anyhow, bail, Context, Result};
use byoridb_bulkloader::key;
use byoridb_bulkloader::loader::{self, Loader, LoaderConfig};
use byoridb_bulkloader::schema::{column_types_from_schema, ColumnTypes};
use byoridb_kvstore::{KVStore, KVStoreOptions, RedbKVStore};
use clap::Parser;
use futures::StreamExt;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "byoridb-bulkloader",
    about = "Offline bulk loader — writes directly into the redb store (server must be stopped)"
)]
struct Args {
    /// redb data directory (the same path the server uses; file is <db>/data.redb).
    #[arg(long)]
    db: PathBuf,

    /// Target space. Must already exist (CREATE SPACE via the server first).
    #[arg(long)]
    space: String,

    /// Node CSV as `tag=path` (repeatable). Loaded in the order given.
    #[arg(long = "node", value_parser = parse_assignment)]
    nodes: Vec<(String, PathBuf)>,

    /// Edge CSV as `edge_type=path` (repeatable). Loaded after all nodes.
    #[arg(long = "edge", value_parser = parse_assignment)]
    edges: Vec<(String, PathBuf)>,

    /// Column holding the original node id (drives vid assignment + preserved as a property).
    #[arg(long, default_value = "id")]
    id_column: String,

    /// Edge column holding the source node's original id.
    #[arg(long, default_value = "src")]
    src_column: String,

    /// Edge column holding the destination node's original id.
    #[arg(long, default_value = "dst")]
    dst_column: String,

    /// Optional edge column holding the ranking (defaults to 0 when absent).
    #[arg(long)]
    ranking_column: Option<String>,

    /// KV pairs per redb transaction. Larger = fewer fsyncs under relaxed durability.
    #[arg(long, default_value_t = 100_000)]
    batch_size: usize,

    /// redb page cache size in MB.
    #[arg(long, default_value_t = 1024)]
    cache_mb: usize,

    /// `relaxed` (no per-commit fsync, checkpoint every 64 commits — fast bulk
    /// load) or `immediate` (fsync per commit — safe but slow).
    #[arg(long, default_value = "relaxed")]
    durability: String,

    /// Abort on duplicate node ids or dangling edge endpoints instead of warn+skip.
    #[arg(long)]
    strict: bool,

    /// After loading, count key prefixes and check them against the live tallies.
    /// Scans the whole keyspace — use only for small/medium loads.
    #[arg(long)]
    verify: bool,
}

fn parse_assignment(s: &str) -> std::result::Result<(String, PathBuf), String> {
    let (k, v) = s
        .split_once('=')
        .ok_or_else(|| format!("expected `name=path`, got '{s}'"))?;
    if k.is_empty() || v.is_empty() {
        return Err(format!("expected non-empty `name=path`, got '{s}'"));
    }
    Ok((k.to_string(), PathBuf::from(v)))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();

    if args.nodes.is_empty() && args.edges.is_empty() {
        bail!("nothing to load: pass at least one --node or --edge");
    }

    let use_fsync = match args.durability.as_str() {
        "relaxed" | "none" => false,
        "immediate" => true,
        other => bail!("--durability must be `relaxed` or `immediate`, got '{other}'"),
    };

    let store = RedbKVStore::open(
        &args.db,
        KVStoreOptions {
            create_if_missing: true,
            cache_size: args.cache_mb * 1024 * 1024,
            use_fsync,
        },
    )
    .map_err(|e| anyhow!("opening redb at {}: {e}", args.db.display()))?;

    // The space DDL must precede the load — the loader only reads metadata.
    if store
        .get(&key::space(&args.space))
        .await
        .map_err(|e| anyhow!("reading space metadata: {e}"))?
        .is_none()
    {
        bail!(
            "space '{}' does not exist — run CREATE SPACE/TAG/EDGE via the server first, \
             then stop the server and re-run the loader",
            args.space
        );
    }

    let cfg = LoaderConfig {
        space: args.space.clone(),
        batch_size: args.batch_size.max(1),
        id_column: args.id_column.clone(),
        src_column: args.src_column.clone(),
        dst_column: args.dst_column.clone(),
        ranking_column: args.ranking_column.clone(),
        strict: args.strict,
    };
    let mut ldr = Loader::new(&store, cfg);

    for (tag, path) in &args.nodes {
        let types = read_column_types(&store, &args.space, true, tag).await?;
        tracing::info!(tag = %tag, file = %path.display(), "loading nodes");
        ldr.load_node_file(tag, path, &types)
            .await
            .with_context(|| format!("loading nodes tag={tag} file={}", path.display()))?;
        let s = ldr.stats();
        tracing::info!(
            vertices = s.vertices,
            duplicate_ids = s.duplicate_ids,
            "nodes progress"
        );
    }

    for (edge_type, path) in &args.edges {
        let types = read_column_types(&store, &args.space, false, edge_type).await?;
        tracing::info!(edge = %edge_type, file = %path.display(), "loading edges");
        ldr.load_edge_file(edge_type, path, &types)
            .await
            .with_context(|| format!("loading edges type={edge_type} file={}", path.display()))?;
        let s = ldr.stats();
        tracing::info!(
            edges = s.edges,
            dangling = s.dangling_edges,
            "edges progress"
        );
    }

    let stats = ldr.finish().await?;
    tracing::info!(
        vertices = stats.vertices,
        tagvid_entries = stats.tagvid_entries,
        edges = stats.edges,
        duplicate_ids = stats.duplicate_ids,
        dangling_edges = stats.dangling_edges,
        "bulk load complete"
    );
    if stats.duplicate_ids > 0 {
        tracing::warn!(
            count = stats.duplicate_ids,
            "duplicate node ids were skipped (idempotent re-load)"
        );
    }
    if stats.dangling_edges > 0 {
        tracing::warn!(
            count = stats.dangling_edges,
            "edges with an endpoint not found among loaded nodes were dropped"
        );
    }

    if args.verify {
        verify(&store, &args.space, &stats).await?;
    }

    Ok(())
}

/// Read the declared column types for a tag or edge from its schema JSON.
/// A missing schema yields an empty map (loader keeps every column as a string).
async fn read_column_types(
    store: &RedbKVStore,
    space: &str,
    is_tag: bool,
    name: &str,
) -> Result<ColumnTypes> {
    let meta_key = if is_tag {
        key::tag(space, name)
    } else {
        key::edge(space, name)
    };
    match store
        .get(&meta_key)
        .await
        .map_err(|e| anyhow!("reading schema for {name}: {e}"))?
    {
        Some(bytes) => {
            let json: serde_json::Value = serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing schema JSON for {name}"))?;
            Ok(column_types_from_schema(&json))
        }
        None => {
            tracing::warn!(
                name = %name,
                kind = if is_tag { "tag" } else { "edge" },
                "schema not found; treating all columns as strings"
            );
            Ok(ColumnTypes::new())
        }
    }
}

/// Count key prefixes via streaming scan and compare to the loader's tallies.
async fn verify(store: &RedbKVStore, space: &str, stats: &loader::LoaderStats) -> Result<()> {
    let vertices = count_prefix(store, &key::vertex_prefix(space)).await?;
    let tagvids = count_prefix(store, &key::tagvid_prefix(space)).await?;
    let edges = count_prefix(store, &key::edge_prefix(space)).await?;
    let in_edges = count_prefix(store, &key::in_edge_prefix(space)).await?;

    tracing::info!(vertices, tagvids, edges, in_edges, "verify counts");

    let mut ok = true;
    if vertices != stats.vertices {
        tracing::error!(
            expected = stats.vertices,
            got = vertices,
            "vertex count mismatch"
        );
        ok = false;
    }
    if tagvids != stats.tagvid_entries {
        tracing::error!(
            expected = stats.tagvid_entries,
            got = tagvids,
            "tagvid count mismatch"
        );
        ok = false;
    }
    if edges != stats.edges {
        tracing::error!(
            expected = stats.edges,
            got = edges,
            "forward edge count mismatch"
        );
        ok = false;
    }
    if in_edges != stats.edges {
        tracing::error!(
            expected = stats.edges,
            got = in_edges,
            "reverse edge count mismatch"
        );
        ok = false;
    }
    if !ok {
        bail!("verification failed — see count mismatches above");
    }
    tracing::info!("verification passed");
    Ok(())
}

/// Count entries under a prefix without materializing them all in memory.
/// NOTE: `vertex_prefix` (`{space}:vertex:`) is also a prefix of nothing else,
/// and `edge_prefix` (`{space}:edge:`) does not overlap `in-edge:` — distinct
/// literal prefixes, so counts don't bleed across keyspaces.
async fn count_prefix(store: &RedbKVStore, prefix: &[u8]) -> Result<u64> {
    let mut stream = store
        .scan_stream(prefix)
        .await
        .map_err(|e| anyhow!("scan_stream failed: {e}"))?;
    let mut n = 0u64;
    while let Some(item) = stream.next().await {
        item.map_err(|e| anyhow!("scan item error: {e}"))?;
        n += 1;
    }
    Ok(n)
}

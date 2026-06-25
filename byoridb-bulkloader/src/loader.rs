// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Core bulk-load logic: stream CSV rows, assign sequential INT64 vids, and
//! write the exact set of KV pairs the INSERT path produces — vertex blob +
//! tag-vid index for nodes, forward + reverse edge for edges — directly into
//! redb in sorted batches.

use crate::key;
use crate::schema::{cell_to_value, ColumnTypes};
use anyhow::{anyhow, bail, Context, Result};
use byoridb_codec::vertex::{EdgeData, TagData, VertexCodec, VertexData};
use byoridb_kvstore::{KVStore, RedbKVStore};
use flate2::read::GzDecoder;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Column-name conventions and batching knobs for one load run.
pub struct LoaderConfig {
    pub space: String,
    /// Number of KV pairs per redb transaction. Larger = fewer fsyncs under
    /// relaxed durability (a checkpoint fires every 64 commits).
    pub batch_size: usize,
    pub id_column: String,
    pub src_column: String,
    pub dst_column: String,
    pub ranking_column: Option<String>,
    /// Abort on a duplicate node id or a dangling edge endpoint instead of
    /// warning and continuing.
    pub strict: bool,
}

#[derive(Debug, Default, Clone)]
pub struct LoaderStats {
    pub vertices: u64,
    pub tagvid_entries: u64,
    pub edges: u64,
    pub duplicate_ids: u64,
    pub dangling_edges: u64,
}

pub struct Loader<'a> {
    store: &'a RedbKVStore,
    cfg: LoaderConfig,
    /// Original (string) id -> assigned sequential vid. Needed to resolve edge
    /// endpoints. Sized for the whole dataset (~89M entries at full scale).
    id_map: HashMap<String, i64>,
    next_vid: i64,
    pending: Vec<(Vec<u8>, Vec<u8>)>,
    stats: LoaderStats,
}

impl<'a> Loader<'a> {
    pub fn new(store: &'a RedbKVStore, cfg: LoaderConfig) -> Self {
        Loader {
            store,
            cfg,
            id_map: HashMap::new(),
            next_vid: 1, // vids start at 1; 0 is the proto/codec default sentinel
            pending: Vec::new(),
            stats: LoaderStats::default(),
        }
    }

    pub fn stats(&self) -> &LoaderStats {
        &self.stats
    }

    /// Assign (or look up) a sequential vid for an original string id.
    /// Returns `(vid, is_new)`.
    fn assign_vid(&mut self, orig_id: &str) -> (i64, bool) {
        if let Some(&v) = self.id_map.get(orig_id) {
            return (v, false);
        }
        let v = self.next_vid;
        self.next_vid += 1;
        self.id_map.insert(orig_id.to_string(), v);
        (v, true)
    }

    fn push(&mut self, k: Vec<u8>, v: Vec<u8>) {
        self.pending.push((k, v));
    }

    async fn maybe_flush(&mut self) -> Result<()> {
        if self.pending.len() >= self.cfg.batch_size {
            self.flush().await?;
        }
        Ok(())
    }

    /// Sort the pending batch by key bytes (so redb inserts land in B-tree
    /// order → append-mostly, minimal page churn) and commit it atomically.
    async fn flush(&mut self) -> Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let mut batch = std::mem::take(&mut self.pending);
        batch.sort_by(|a, b| a.0.cmp(&b.0));
        self.store
            .batch_put(batch)
            .await
            .map_err(|e| anyhow!("batch_put failed: {e}"))?;
        Ok(())
    }

    /// Load one node CSV under a single tag. Every column (including the id
    /// column) is preserved as a property; the id column additionally drives
    /// vid assignment.
    pub async fn load_node_file(
        &mut self,
        tag: &str,
        path: &Path,
        types: &ColumnTypes,
    ) -> Result<()> {
        let mut rdr = open_csv(path)?;
        let headers = rdr
            .headers()
            .with_context(|| format!("reading headers of {}", path.display()))?
            .clone();
        let id_idx = headers
            .iter()
            .position(|h| h == self.cfg.id_column)
            .ok_or_else(|| {
                anyhow!(
                    "node file {} has no id column '{}'",
                    path.display(),
                    self.cfg.id_column
                )
            })?;

        let mut record = csv::StringRecord::new();
        while rdr
            .read_record(&mut record)
            .with_context(|| format!("reading {}", path.display()))?
        {
            let orig_id = record.get(id_idx).unwrap_or("");
            if orig_id.is_empty() {
                if self.cfg.strict {
                    bail!("empty id in {} (tag {tag})", path.display());
                }
                continue;
            }
            let (vid, is_new) = self.assign_vid(orig_id);
            if !is_new {
                self.stats.duplicate_ids += 1;
                if self.cfg.strict {
                    bail!("duplicate node id '{orig_id}' (tag {tag})");
                }
                // Re-loading the same id: skip re-writing (idempotent).
                continue;
            }

            let mut properties = HashMap::new();
            for (i, h) in headers.iter().enumerate() {
                let cell = record.get(i).unwrap_or("");
                let v = cell_to_value(cell, types.get(h).map(String::as_str));
                if !matches!(v, byoridb_common::Value::Null(_)) {
                    properties.insert(h.to_string(), v);
                }
            }

            let blob = VertexCodec::encode_vertex(&VertexData {
                vid,
                tags: vec![TagData {
                    name: tag.to_string(),
                    properties,
                }],
            })
            .map_err(|e| anyhow!("encode_vertex(vid={vid}): {e}"))?;

            self.push(key::vertex(&self.cfg.space, vid), blob);
            self.push(key::tagvid(&self.cfg.space, tag, vid), Vec::new());
            self.stats.vertices += 1;
            self.stats.tagvid_entries += 1;
            self.maybe_flush().await?;
        }
        self.flush().await?;
        Ok(())
    }

    /// Load one edge CSV under a single edge type. src/dst columns are resolved
    /// to vids via the id map built during node loading. Both the forward
    /// (`edge:`) and reverse (`in-edge:`) keys get the same denormalized payload.
    pub async fn load_edge_file(
        &mut self,
        edge_type: &str,
        path: &Path,
        types: &ColumnTypes,
    ) -> Result<()> {
        if edge_type == "sameAs" {
            // The engine treats edge type exactly `sameAs` as owl:sameAs and
            // expects union-find side stores the loader does not build. Use
            // `same_as` (underscore) instead. Reject to prevent silent breakage.
            bail!("edge type 'sameAs' is reserved for owl:sameAs merge; use 'same_as'");
        }
        let mut rdr = open_csv(path)?;
        let headers = rdr
            .headers()
            .with_context(|| format!("reading headers of {}", path.display()))?
            .clone();
        let pos = |name: &str| headers.iter().position(|h| h == name);
        let src_idx = pos(&self.cfg.src_column).ok_or_else(|| {
            anyhow!(
                "edge file {} has no src column '{}'",
                path.display(),
                self.cfg.src_column
            )
        })?;
        let dst_idx = pos(&self.cfg.dst_column).ok_or_else(|| {
            anyhow!(
                "edge file {} has no dst column '{}'",
                path.display(),
                self.cfg.dst_column
            )
        })?;
        let rank_idx = self.cfg.ranking_column.as_deref().and_then(pos);

        let mut record = csv::StringRecord::new();
        while rdr
            .read_record(&mut record)
            .with_context(|| format!("reading {}", path.display()))?
        {
            let src_orig = record.get(src_idx).unwrap_or("");
            let dst_orig = record.get(dst_idx).unwrap_or("");
            let (src_vid, dst_vid) = match (
                self.id_map.get(src_orig).copied(),
                self.id_map.get(dst_orig).copied(),
            ) {
                (Some(s), Some(d)) => (s, d),
                _ => {
                    self.stats.dangling_edges += 1;
                    if self.cfg.strict {
                        bail!(
                            "dangling edge {edge_type}: '{src_orig}'->'{dst_orig}' \
                             (endpoint not found among loaded nodes)"
                        );
                    }
                    continue;
                }
            };

            let ranking = rank_idx
                .and_then(|i| record.get(i))
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(0);

            let mut properties = HashMap::new();
            for (i, h) in headers.iter().enumerate() {
                if i == src_idx || i == dst_idx || Some(i) == rank_idx {
                    continue;
                }
                let cell = record.get(i).unwrap_or("");
                let v = cell_to_value(cell, types.get(h).map(String::as_str));
                if !matches!(v, byoridb_common::Value::Null(_)) {
                    properties.insert(h.to_string(), v);
                }
            }

            let blob = VertexCodec::encode_edge(&EdgeData {
                src_vid,
                dst_vid,
                edge_type: edge_type.to_string(),
                ranking,
                properties,
            })
            .map_err(|e| anyhow!("encode_edge({src_vid}->{dst_vid}): {e}"))?;

            self.push(
                key::edge_data(&self.cfg.space, src_vid, edge_type, dst_vid, ranking),
                blob.clone(),
            );
            self.push(
                key::in_edge_data(&self.cfg.space, dst_vid, edge_type, src_vid, ranking),
                blob,
            );
            self.stats.edges += 1;
            self.maybe_flush().await?;
        }
        self.flush().await?;
        Ok(())
    }

    /// Flush any remaining batch and force a durable (fsync) checkpoint so the
    /// relaxed-durability commits are guaranteed on disk before exit.
    pub async fn finish(mut self) -> Result<LoaderStats> {
        self.flush().await?;
        self.store
            .force_checkpoint()
            .await
            .map_err(|e| anyhow!("final checkpoint failed: {e}"))?;
        Ok(self.stats)
    }
}

/// Open a CSV reader, transparently gunzipping `.gz` files.
fn open_csv(path: &Path) -> Result<csv::Reader<Box<dyn Read>>> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let reader: Box<dyn Read> = if path.extension().and_then(|e| e.to_str()) == Some("gz") {
        Box::new(GzDecoder::new(file))
    } else {
        Box::new(file)
    };
    Ok(csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(reader))
}

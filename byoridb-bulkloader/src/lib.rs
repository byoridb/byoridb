// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Offline bulk loader library: key builders, schema/type handling, and the
//! core load engine. The `byoridb-bulkloader` binary is a thin CLI over this.
//!
//! This loader bypasses the nGQL DML path, so it also bypasses write-through
//! maintenance for secondary structures owned by the executor. Any bulk loader
//! or direct-KV writer that creates/updates/deletes vertices must either update
//! text-search indexes itself using the same key/stats/manifest contract as
//! `executor::text_search`, or run `REBUILD TEXT INDEX ON <tag>(<prop>)` before
//! serving search traffic.
//!
//! See [`loader::Loader`] for the load engine and the binary's `--help` for
//! the command-line surface.

pub mod key;
pub mod loader;
pub mod schema;

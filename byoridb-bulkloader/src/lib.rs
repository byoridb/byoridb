// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Offline bulk loader library: key builders, schema/type handling, and the
//! core load engine. The `byoridb-bulkloader` binary is a thin CLI over this.
//!
//! See [`loader::Loader`] for the load engine and the binary's `--help` for
//! the command-line surface.

pub mod key;
pub mod loader;
pub mod schema;

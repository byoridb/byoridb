//! Shared `--version` string for this package's binaries.
//!
//! Included with `#[path]` by both `main.rs` and `backup_cli.rs`, which are
//! separate binary roots in one package and so cannot share a module the usual
//! way. Duplicating the string instead is how `byoridb-backup` came to report a
//! hardcoded `0.1.0` while the package was at `0.3.3`.

/// Crate version, the commit it was built from, and the build profile. The two
/// extra values come from `build.rs`. The version is the release line that
/// `.github/workflows/autorelease.yml` maintains — one patch per push to
/// `main` — and the SHA pins the exact commit within it, including the `-dirty`
/// case a tag cannot express.
pub const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (commit ",
    env!("BYORIDB_GIT_SHA"),
    ", ",
    env!("BYORIDB_BUILD_PROFILE"),
    ")"
);

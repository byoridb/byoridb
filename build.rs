//! Build script for the `byoridb` binaries.
//!
//! Records the commit each binary was built from so a deployed artifact can
//! identify itself. The released version comes from `Cargo.toml`, which
//! `.github/workflows/autorelease.yml` bumps once per push to `main`; the SHA
//! is what distinguishes builds a version alone cannot — a local build from a
//! modified tree, or any commit that was never tagged.

use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rustc-env=BYORIDB_GIT_SHA={}", git_revision());
    println!(
        "cargo:rustc-env=BYORIDB_BUILD_PROFILE={}",
        std::env::var("PROFILE").unwrap_or_else(|_| "unknown".to_string())
    );

    // `--git-path` resolves through worktrees and submodules, where `.git` is a
    // file rather than a directory. Source archives have no git metadata at all,
    // so the directive is emitted only when the path really exists.
    if let Some(head) = git(&["rev-parse", "--git-path", "HEAD"]) {
        if Path::new(&head).exists() {
            println!("cargo:rerun-if-changed={head}");
        }
    }
}

/// Short SHA with a `-dirty` marker for uncommitted changes, or `unknown`
/// outside a git checkout. A build from a modified tree must not claim to be
/// the commit it was branched from.
fn git_revision() -> String {
    let Some(sha) = git(&["rev-parse", "--short=12", "HEAD"]) else {
        return "unknown".to_string();
    };
    match git(&["status", "--porcelain"]) {
        Some(status) if !status.is_empty() => format!("{sha}-dirty"),
        _ => sha,
    }
}

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8(output.stdout).ok()?.trim().to_string())
}

//! `byoridb-server` argument handling.
//!
//! The binary previously ignored argv entirely, so `--version` and any typo
//! started a full server against default configuration. These tests pin the
//! three things that has to stop doing: identify itself, refuse unknown flags,
//! and touch nothing on disk while doing either.

use std::path::Path;
use std::process::{Command, Output};

/// Run the server binary in an empty directory with every variable that could
/// steer it removed, so a data directory appearing under `dir` can only have
/// been created by this run resolving the default `data/storage` path.
fn run_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_byoridb-server"))
        .args(args)
        .current_dir(dir)
        .env_remove("BYORIDB_ROOT_PASSWORD")
        .env_remove("BYORIDB__STORAGE__DATA_PATHS")
        .env_remove("BYORIDB__SERVER__GRAPH_ADDR")
        .env_remove("BYORIDB__SERVER__HTTP_ADDR")
        .env_remove("BYORIDB__CLUSTER__PEERS")
        .output()
        .expect("failed to execute byoridb-server")
}

#[test]
fn version_reports_crate_version_and_commit_without_side_effects() {
    let dir = tempfile::tempdir().unwrap();
    let output = run_in(dir.path(), &["--version"]);

    assert!(
        output.status.success(),
        "--version must exit 0, got {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(env!("CARGO_PKG_VERSION")),
        "--version must report the crate version, got: {stdout}"
    );
    assert!(
        stdout.contains("commit "),
        "--version must report the build commit so a deployed artifact is \
         identifiable, got: {stdout}"
    );

    // No credential was supplied, so reaching credential resolution would have
    // failed the run; reaching storage would have created the default path.
    assert!(
        !dir.path().join("data").exists(),
        "--version must not open storage"
    );
}

#[test]
fn help_lists_configuration_without_side_effects() {
    let dir = tempfile::tempdir().unwrap();
    let output = run_in(dir.path(), &["--help"]);

    assert!(output.status.success(), "--help must exit 0");

    let stdout = String::from_utf8_lossy(&output.stdout);
    for expected in [
        "BYORIDB_ROOT_PASSWORD",
        "BYORIDB__STORAGE__DATA_PATHS",
        "BYORIDB__CLUSTER__PEERS",
    ] {
        assert!(
            stdout.contains(expected),
            "--help must document {expected}, got: {stdout}"
        );
    }

    assert!(
        !dir.path().join("data").exists(),
        "--help must not open storage"
    );
}

#[test]
fn unknown_flag_is_rejected_instead_of_starting_a_server() {
    let dir = tempfile::tempdir().unwrap();
    let output = run_in(dir.path(), &["--nonsense"]);

    assert!(
        !output.status.success(),
        "an unrecognized flag must not start a server"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("nonsense"),
        "the error must name the offending argument, got: {stderr}"
    );

    assert!(
        !dir.path().join("data").exists(),
        "a rejected invocation must not open storage"
    );
}

#[test]
fn unknown_flag_is_rejected_even_with_a_valid_root_password() {
    // The credential gate is not the thing catching bad arguments: with a valid
    // password a typo previously booted a real server.
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_byoridb-server"))
        .arg("--port=9999")
        .current_dir(dir.path())
        .env("BYORIDB_ROOT_PASSWORD", "test-only-secret")
        .env_remove("BYORIDB__STORAGE__DATA_PATHS")
        .output()
        .expect("failed to execute byoridb-server");

    assert!(
        !output.status.success(),
        "an unrecognized flag must be rejected before the server starts"
    );
    assert!(
        !dir.path().join("data").exists(),
        "a rejected invocation must not open storage"
    );
}

# Contributing

[한국어](../ko/development/contributing.html)

Contributions to ByoriDB are welcome. This page summarizes the workflow that is
actually enforced by the repository. Read the root `CONTRIBUTING.md` as well.
If your local checkout includes an `AGENTS.md`, follow those repository-local
agent instructions for AI-assisted changes.

## Prerequisites

- Git
- Rust 1.90, selected by `rust-toolchain.toml`
- `protobuf-compiler` (`protoc`) for gRPC code generation
- Linux or macOS

The production KV engine is pure Rust, so RocksDB/C++ is not a prerequisite.

## Set up a checkout

```bash
git clone https://github.com/byoridb/byoridb.git
cd byoridb

# Make the repository's formatting hook active.
bash scripts/setup-hooks.sh

cargo build --locked --workspace
```

Create a focused branch from the current `main` branch and open the pull
request back to `main`. The repository has no active `develop` branch, so do
not base the workflow on one.

## Workspace layout

Choose the crate that owns the behavior:

| Change | Location |
|---|---|
| Shared values and graph types | `byoridb-common` |
| Persistent KV behavior and backup implementation | `byoridb-kvstore` |
| Vertex/edge/row encoding | `byoridb-codec` |
| Storage, indexes, partitions, or Raft | `byoridb-storage` |
| Metadata services | `byoridb-meta` |
| Lexer, AST, or grammar | `byoridb-parser` |
| Query plans and execution | `byoridb-executor` |
| Authentication, sessions, gRPC, or HTTP | `byoridb-graph` |
| Rust client or CLI | `byoridb-client` |
| Offline import | `byoridb-bulkloader` |
| Server or backup binary | root `src/` |

Do not recreate the removed `byoridb-core` crate. Keep the executor router in
`byoridb-executor/src/executor/mod.rs` small; add purpose-specific modules for
new execution logic.

## Code conventions

- Use `snake_case` for functions, variables, and modules; `PascalCase` for
  types; and `SCREAMING_SNAKE_CASE` for constants.
- Use typed `thiserror` errors in library crates. At service/binary boundaries,
  add context with `anyhow`.
- Do not add `unwrap()` or `expect()` to production code.
- Use structured `tracing` fields instead of `println!`, `eprintln!`, or `dbg!`
  in production code.
- Register a dependency in root `[workspace.dependencies]` first and reference
  it with `workspace = true` from member crates.
- Add inline unit tests for new behavior and integration tests where a
  cross-crate path matters.

Example library error:

```rust
#[derive(Debug, thiserror::Error)]
pub enum ExecutorError {
    #[error("space not selected")]
    NoSpace,
}
```

Example structured log:

```rust
tracing::info!(space = %space_name, "query executed");
```

## Required local checks

Run the same broad gates expected for a pull request:

```bash
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features -- --test-threads=1
cargo build --locked --workspace --release
```

Integration tests must run serially because temporary redb databases can
otherwise contend for files. Useful narrower commands while iterating include:

```bash
cargo test --locked --package byoridb-parser
cargo test --locked --package byoridb-executor <test-name>
```

The GitHub workflow also runs a pinned RustSec dependency audit. Its release
build waits for check, format, Clippy, test, and audit jobs to pass.

## High-risk areas

Some modules require additional regression gates.

### Query correctness regressions

If a change touches the H-series paths described in `docs/PLAN.md`, rerun the
H-1 through H-6 regression coverage, including the multi-pattern MATCH tests.
Do not reintroduce a fallback `space_id`, `tag_id`, or `edge_id` of zero.

### Temporal storage

Changes to current/history storage, temporal DML, parsing, or planning must
preserve both the current view and history:

```bash
cargo test --locked -p byoridb-kvstore --test temporal
cargo test --locked -p byoridb-parser fetch_as_of
cargo test --locked -p byoridb-executor fetch_as_of
```

Then run the full serial workspace suite. Verify vertex and edge `AS OF`,
tombstones, same-millisecond writes, and atomic current/history application as
applicable.

### Raft and distributed code

Understand the custom Raft state machine, log, snapshots, membership, and
network driver before changing `byoridb-storage/src/raft/`. Run:

```bash
cargo test --locked -p byoridb --all-features \
  --test distributed_e2e_test -- --test-threads=1
```

Passing component tests does not make the unfinished cluster launcher a
production-ready deployment; document that boundary explicitly.

### Authentication and authorization

For user, role, session, or protocol changes, test both HTTP and gRPC behavior,
durable-user cache reconciliation, session invalidation, compound statements,
and `PROFILE` authorization. Never include credentials or raw session IDs in
logs, metrics, diagnostics, or error responses.

## Documentation changes

English pages under `book/src/` are canonical. Maintain the corresponding
Korean page under `book/src.ko/` and keep commands, feature status, and
limitations semantically aligned. Do not publish measured performance numbers
without a reproducible environment and result artifact.

When mdBook is installed, build both language trees using the repository's
documentation workflow/configuration.

## Pull requests

Keep a pull request focused and include:

- the problem and the chosen behavior;
- security, compatibility, and data-migration implications;
- tests added and exact commands run;
- documentation updates for user-visible behavior;
- links to related issues when applicable.

Do not commit `.env` files, credentials, generated database files, or
machine-specific build output. The pull-request template and CI results are the
authoritative submission checklist.

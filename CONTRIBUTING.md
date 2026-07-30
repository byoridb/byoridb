# Contributing to ByoriDB

[English (default)](CONTRIBUTING.md) | [한국어](CONTRIBUTING.ko.md)

Thank you for helping improve ByoriDB. Bug reports, focused fixes, tests, and
documentation improvements are welcome.

Do not open a public issue for a suspected vulnerability. Follow the private
reporting process in [SECURITY.md](SECURITY.md).

## Before you start

- Search existing issues and pull requests to avoid duplicate work.
- For a large feature, storage-format change, query-language change, or
  distributed-systems change, open an issue first and describe the proposed
  behavior and compatibility impact.
- Read [docs/PLAN.md](docs/PLAN.md) for current implementation constraints and
  known regression areas.
- Keep changes focused. Do not mix unrelated formatting, refactors, and feature
  work in one pull request.

## Development environment

Required tools:

- Linux or macOS
- Rust 1.90 from `rust-toolchain.toml`
- `protobuf-compiler` for gRPC code generation

The embedded storage backend is redb and does not require a C++ toolchain.

```bash
git clone https://github.com/byoridb/byoridb.git
cd byoridb
cargo build --locked --workspace
```

Enable the repository's formatting pre-commit hook if desired:

```bash
./scripts/setup-hooks.sh
```

## Repository layout

| Path | Responsibility |
|---|---|
| `src/` | `byoridb-server` and `byoridb-backup` entry points |
| `byoridb-common/` | Shared graph values, records, datasets, and crypto helpers |
| `byoridb-kvstore/` | redb current view, temporal history, and backup support |
| `byoridb-codec/` | Binary row encoding and decoding |
| `byoridb-storage/` | Vertex/edge storage, partitioning, and custom Raft |
| `byoridb-meta/` | Spaces, schemas, partitions, and metadata services |
| `byoridb-parser/` | Lexer, AST, and nGQL-style parsers |
| `byoridb-executor/` | Plans, query execution, inference, paths, and recommendation |
| `byoridb-graph/` | HTTP/gRPC, authentication, sessions, and query coordination |
| `byoridb-client/` | Rust client and `byoridb-cli` |
| `byoridb-bulkloader/` | Bulk-loading binary and library |
| `tests/` | Workspace integration and distributed tests |
| `book/` | mdBook user and operations documentation |
| `docs/` | Implementation status, decisions, and migration plans |

The removed `byoridb-core/` crate must not be recreated.

## Code conventions

- Follow Rust naming conventions: `snake_case` for functions and modules,
  `PascalCase` for types, and `SCREAMING_SNAKE_CASE` for constants.
- Library crates should expose typed errors with `thiserror`; service and binary
  boundaries may add context with `anyhow`.
- Do not add `unwrap()` or `expect()` to production paths. They are allowed in
  tests when the failure message remains useful.
- Use structured `tracing` events in production code. Do not commit `println!`,
  `eprintln!`, or `dbg!` in production paths.
- Register shared third-party dependencies under `[workspace.dependencies]` in
  the root `Cargo.toml`, then reference them with `workspace = true` from member
  crates.
- Add focused tests for new behavior. Prefer an inline `#[cfg(test)]` module for
  unit behavior and `tests/` for cross-crate behavior.
- Keep `byoridb-executor/src/executor/mod.rs` as a router; put substantial new
  execution logic in a purpose-specific module.
- Never commit credentials, `.env` files, private data, or generated databases.

## Documentation conventions

English is the canonical documentation language. When changing a public English
document, update its Korean mirror (`*.ko.md`) in the same pull request. Keep
commands, identifiers, paths, and query examples identical between languages.

Document behavior that exists in the current code. Mark incomplete or
experimental behavior explicitly; in particular, do not describe the current
multi-node launcher as production-ready.

## Required checks

Check, format, and lint the complete workspace:

```bash
cargo check --locked --workspace --all-targets --all-features
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
```

Run all tests serially, then confirm that the release workspace builds. Serial
execution is required because temporary redb databases can contend for files
and locks:

```bash
cargo test --locked --workspace --all-features -- --test-threads=1
cargo build --locked --workspace --release
```

CI also runs a RustSec dependency audit against the committed `Cargo.lock`.

Useful narrower commands while iterating:

```bash
cargo test --locked --package byoridb-executor
cargo test --locked --package byoridb-graph
cargo test --locked --package byoridb-kvstore --test temporal
```

The full workspace test and release-build commands are still the final local
gates.

### High-risk areas

- **Space/tag/edge identifiers:** review the H-series regressions in
  `docs/PLAN.md` and rerun their regression tests when changing schema keys,
  planning, lookup, fetch, traversal, or multi-pattern matching.
- **Temporal paths:** verify current-view behavior and history together. Run the
  temporal KV test, parser/executor `fetch_as_of` tests, and the full serial
  workspace suite.
- **Custom Raft:** understand log, snapshot, and membership behavior before
  editing `byoridb-storage/src/raft/`; run
  `tests/distributed_e2e_test.rs` with all features.
- **Authentication and authorization:** add a negative test for every privilege
  boundary, including nested/compound statements and session invalidation.

## Pull-request workflow

1. Fork the repository and create a focused branch.
2. Implement the smallest coherent change with tests and documentation.
3. Run the required checks above.
4. Use a concise conventional commit such as
   `fix(executor): preserve temporal history`.
5. Push your branch and open a pull request using the repository template.

In the pull request, explain:

- the problem and intended behavior;
- important design choices and compatibility risks;
- exactly which commands you ran and their results;
- migration, deployment, or rollback considerations;
- documentation changed with the implementation.

All contributions are submitted under the repository's
[Apache License 2.0](LICENSE) unless explicitly stated otherwise.

## Getting help

Open a GitHub issue for reproducible bugs and focused feature discussions. Include
the ByoriDB revision, operating system, relevant configuration with secrets
removed, a minimal query sequence, and the observed error. Use the private
security process for anything that may expose credentials, data, or an access
control bypass.

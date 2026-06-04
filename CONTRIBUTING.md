# Contributing to ByoriDB

Thank you for your interest in contributing to ByoriDB! We welcome contributions from the community.

## Getting Started

### Prerequisites

- **Rust**: We use the latest stable version of Rust. Install it via [rustup](https://rustup.rs/).
- **C++ Build Tools**: Required for compiling RocksDB (e.g., `cmake`, `clang`, `gcc`).

### Building the Project

1. Clone the repository:
   ```bash
   git clone https://github.com/byoridb/byoridb.git
   cd byoridb
   ```

2. Build the project:
   ```bash
   cargo build
   ```

3. Set up git hooks (recommended):
   ```bash
   ./scripts/setup-hooks.sh
   ```
   This enables pre-commit hooks that automatically check code formatting.

### Running Tests

We encourage running tests before submitting a Pull Request.

```bash
cargo test
```

To run a specific test:

```bash
cargo test --package byoridb --test test_name
```

## Code Style

We follow standard Rust coding conventions.

- **Formatting**: Please ensure your code is formatted with `rustfmt`.
  ```bash
  cargo fmt --all
  ```

- **Clippy**: Run clippy to catch common mistakes.
  ```bash
  cargo clippy --all-targets --all-features -- -D warnings
  ```

## Development Workflow

1. Fork the repository.
2. Create a new branch for your feature or fix (`git checkout -b feature/my-feature`).
3. Commit your changes (`git commit -am 'Add some feature'`).
4. Push to the branch (`git push origin feature/my-feature`).
5. Open a Pull Request.

## Project Structure

- `byoridb-common`: Core data types (Value, Vertex, Edge, DataSet).
- `byoridb-kvstore`: KV storage layer with RocksDB + WAL.
- `byoridb-codec`: Row encoding/decoding with schema versioning.
- `byoridb-storage`: Storage service, Raft consensus, indexing.
- `byoridb-meta`: Metadata management, partition allocation.
- `byoridb-parser`: nGQL query language parser.
- `byoridb-executor`: Query planning and execution engine.
- `byoridb`: Graph service, HTTP/gRPC server.
- `byoridb-client`: Client library and CLI.

## Getting Help

If you have questions, please open an issue or join our community discussions.

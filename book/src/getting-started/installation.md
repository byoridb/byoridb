[한국어](../ko/getting-started/installation.html)

# Installation

ByoriDB is currently built from source. The repository pins Rust 1.90, so a
`rustup` installation will select the expected compiler automatically.

## Supported systems

- Linux
- macOS

Windows is not currently supported. ByoriDB uses the pure-Rust `redb` storage
engine, so RocksDB and a C++ RocksDB toolchain are not required.

## Build requirements

- Git
- Rust 1.90 with Cargo, rustfmt, and Clippy
- `protoc` (`protobuf-compiler`) for gRPC code generation
- The platform's standard C build tools, which some transitive dependencies may
  compile during a source build

Install the common prerequisites on Ubuntu or Debian:

```bash
sudo apt update
sudo apt install -y build-essential protobuf-compiler
```

On macOS:

```bash
xcode-select --install
brew install protobuf
```

Install Rust with [rustup](https://rustup.rs/) if it is not already available:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

## Build ByoriDB

```bash
git clone https://github.com/byoridb/byoridb.git
cd byoridb
cargo build --workspace --release
```

The principal binaries are:

| Binary | Output path | Purpose |
| --- | --- | --- |
| `byoridb-server` | `target/release/byoridb-server` | Standalone server |
| `byoridb-cli` | `target/release/byoridb-cli` | gRPC command-line client |
| `byoridb-backup` | `target/release/byoridb-backup` | Backup utility |

To build only the server or CLI:

```bash
cargo build --release --bin byoridb-server
cargo build --release -p byoridb-client --bin byoridb-cli
```

## Verify the checkout

Integration tests must run serially because temporary redb databases can
otherwise contend for file locks:

```bash
cargo test --workspace --all-features -- --test-threads=1
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

## Start the server

The standalone binary refuses to start without a non-blank root password. It
never generates and prints a recoverable password to the logs.

```bash
export BYORIDB_ROOT_PASSWORD='replace-with-a-long-random-secret'
./target/release/byoridb-server
```

See [Configuration](./configuration.md) before exposing a server outside a
development machine. The built-in listeners do not provide TLS.

## Troubleshooting

If a build reports that `protoc` is missing, install `protobuf-compiler` on
Linux or `protobuf` through Homebrew on macOS. If a release build exhausts local
memory, reduce Cargo's parallelism:

```bash
cargo build --workspace --release -j 2
```

[한국어](../ko/getting-started/installation.html)

# Installation

Tagged [GitHub releases](https://github.com/byoridb/byoridb/releases) publish
prebuilt archives for Linux x86_64/arm64 and macOS x86_64/arm64. Releases made
before the arm64 workflow landed do not retroactively gain that artifact. A
tagged release can lag the current `main` branch; the instructions below build
the exact checkout. The repository pins Rust 1.90, so a `rustup` installation
will select the expected compiler automatically.

## Supported systems

- Linux
- macOS

Windows is not currently supported. ByoriDB uses the pure-Rust `redb` storage
engine, so RocksDB and a C++ RocksDB toolchain are not required.

The release workflow builds Linux arm64 natively on GitHub's arm64 runner. It
also refuses a tagged macOS release unless all Developer ID signing and App
Store Connect notarization credentials listed below are configured. Existing
v0.3.3 and older macOS archives predate that gate and remain unsigned; build
those tags from source instead of bypassing Gatekeeper.

Release maintainers must configure these repository secrets before creating a
new tag:

- `MACOS_CERTIFICATE_P12_BASE64`, `MACOS_CERTIFICATE_PASSWORD`, and
  `MACOS_SIGNING_IDENTITY` for a Developer ID Application certificate.
- `APPLE_API_KEY_P8_BASE64`, `APPLE_API_KEY_ID`, and `APPLE_API_ISSUER_ID` for
  `xcrun notarytool`.

The workflow signs every macOS executable with the hardened runtime and secure
timestamp, verifies each signature, and waits for Apple notarization to succeed
before packaging. Missing credentials fail the macOS build rather than silently
publishing another unsigned archive.

Current release archives also contain binaries only and omit `LICENSE` and
`NOTICES.md`; packaging those files remains open in
[issue #28](https://github.com/byoridb/byoridb/issues/28). Review the repository
license and notices before redistributing an archive.

## Build requirements

- Git
- Rust 1.90 with Cargo, rustfmt, and Clippy
- `protoc` (`protobuf-compiler`) for gRPC code generation
- The platform's standard C build tools, which some transitive dependencies may
  compile during a source build

Install the common prerequisites on Ubuntu or Debian:

```bash
sudo apt update
sudo apt install -y build-essential pkg-config libssl-dev protobuf-compiler
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
cargo build --locked --workspace --release
```

The principal binaries are:

| Binary | Output path | Purpose |
| --- | --- | --- |
| `byoridb-server` | `target/release/byoridb-server` | Standalone server |
| `byoridb-cli` | `target/release/byoridb-cli` | gRPC command-line client |
| `byoridb-backup` | `target/release/byoridb-backup` | Backup utility |

To build only the server or CLI:

```bash
cargo build --locked --release -p byoridb --bin byoridb-server
cargo build --locked --release -p byoridb-client --bin byoridb-cli
```

## Verify the checkout

Integration tests must run serially because temporary redb databases can
otherwise contend for file locks:

```bash
cargo test --locked --workspace --all-features -- --test-threads=1
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
```

## Start the server

The standalone binary refuses to start without a non-empty root password. It
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
cargo build --locked --workspace --release -j 2
```

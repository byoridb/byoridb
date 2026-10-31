[한국어](../ko/getting-started/installation.html)

# Installation

The [latest release page](https://github.com/byoridb/byoridb/releases/latest)
provides prebuilt archives for Linux x86_64/arm64 and macOS x86_64/arm64.
Each archive contains the server, CLI, backup tool, license and notices. Its
release workflow runs authenticated HTTP/gRPC queries, graceful restart and
backup/restore against the extracted binaries before publication.
Older releases do not retroactively gain new checks, platforms or
signatures. A tagged release can lag `main`; the source-build instructions below
build the exact checkout. The repository pins Rust 1.90, so a `rustup`
installation will select the expected compiler automatically.

## Supported systems

- Linux
- macOS

Windows is not currently supported. ByoriDB uses the pure-Rust `redb` storage
engine, so RocksDB and a C++ RocksDB toolchain are not required.

The **v0.4.23 Linux prebuilt archives** for both x86_64 and arm64 require
**glibc 2.39 or newer** to run all three tools, such as Ubuntu 24.04. The verified
ELF requirements are glibc 2.34 for the server and backup tool, and 2.39 for the
CLI. These are requirements of this release's archives, not an ABI promise for
future versions. On an older distribution, build from source there or use the
separately built and tested source Docker image. Review each release's runtime
requirements before upgrading.

The release workflow builds Linux arm64 natively on GitHub's arm64 runner.

Every macOS binary in a new tagged archive — `byoridb-server`, `byoridb-cli`,
and `byoridb-backup` — is signed with a Developer ID Application certificate
under the hardened runtime and submitted to Apple for notarization. The release
fails rather than publishing if notarization returns anything other than
`Accepted`. You can confirm what you downloaded:

```bash
codesign --verify --strict --verbose=2 byoridb-server
codesign --display --verbose=2 byoridb-server | grep '^Authority'
```

The notarization ticket is **not stapled** to these binaries. `xcrun stapler`
writes tickets only into `.app`, `.dmg`, and `.pkg` containers, not into a bare
executable inside a tarball, so Gatekeeper resolves the ticket from Apple over
the network on first launch. On a machine with no outbound network access that
lookup cannot happen and Gatekeeper may still refuse; build from source there.

**v0.3.3 and earlier archives are unsigned and unnotarized.** They are not
re-signed retroactively.

The release workflow includes `LICENSE` and `NOTICES.md` at the root of every
new tagged archive and verifies both files before publishing it. Published
v0.3.3 and older archives predate this check and are not changed retroactively;
review the repository license and notices before redistributing those legacy
archives.

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
does not impose a separate strength check on whitespace-only values and never
generates and prints a recoverable password to the logs.

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

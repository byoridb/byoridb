# Installation

## System Requirements

### Supported Platforms
- Linux (Ubuntu 20.04+, CentOS 7+, Debian 10+)
- macOS (10.15+)

> **Note:** Windows is not currently supported.

### Hardware Requirements
- CPU: 2+ cores recommended
- Memory: 4GB+ recommended
- Disk: SSD recommended for production

### Software Dependencies
- Rust 1.90 or later
- protobuf-compiler (for gRPC codegen)
- pkg-config

The storage engine is pure-Rust (redb), so **no C++ toolchain** (cmake/clang) is
required. `build-essential`/`pkg-config` cover the few native crates (zstd, openssl).

## Install Dependencies

### Ubuntu/Debian

```bash
sudo apt update
sudo apt install -y build-essential pkg-config protobuf-compiler
```

### macOS

```bash
xcode-select --install
brew install protobuf
```

### CentOS/RHEL

```bash
sudo yum groupinstall -y "Development Tools"
sudo yum install -y protobuf-compiler
```

## Install Rust

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env
```

Verify installation:

```bash
rustc --version
cargo --version
```

## Build ByoriDB

### Clone Repository

```bash
git clone https://github.com/byoridb/byoridb.git
cd byoridb
```

### Debug Build

```bash
cargo build
```

### Release Build (Recommended)

```bash
cargo build --release
```

The release build enables LTO (Link-Time Optimization) for better performance.

### Build Artifacts

After building, you'll find:

| Binary | Location | Description |
|--------|----------|-------------|
| `byoridb-server` | `target/release/` | Standalone server |
| `byoridb-cli` | `target/release/` | CLI client |

## Verify Installation

```bash
# Run tests
cargo test

# Start server
./target/release/byoridb-server

# In another terminal, connect with CLI
./target/release/byoridb-cli
```

## Troubleshooting

### Protobuf Compiler Not Found

If the gRPC build fails with a missing `protoc`:

```bash
# Ubuntu/Debian
sudo apt install -y protobuf-compiler

# macOS
brew install protobuf
```

### Linking Errors

```bash
# Ensure pkg-config can find libraries
export PKG_CONFIG_PATH=/usr/local/lib/pkgconfig
```

### Out of Memory During Build

```bash
# Limit parallel jobs
cargo build --release -j 2
```

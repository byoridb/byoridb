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
- Rust 1.75 or later
- C++ compiler (clang 10+ or gcc 7+)
- CMake 3.10+
- pkg-config

## Install Dependencies

### Ubuntu/Debian

```bash
sudo apt update
sudo apt install -y build-essential cmake pkg-config clang
```

### macOS

```bash
xcode-select --install
brew install cmake
```

### CentOS/RHEL

```bash
sudo yum groupinstall -y "Development Tools"
sudo yum install -y cmake3 clang
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

### RocksDB Compilation Errors

If you see RocksDB compilation errors:

```bash
# Ubuntu/Debian
sudo apt install -y libclang-dev

# macOS
brew install llvm
export LIBCLANG_PATH=$(brew --prefix llvm)/lib
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

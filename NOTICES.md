# Open Source Software Notices

This project is built using the following open source software:

## RocksDB
ByoriDB leverages RocksDB as its underlying key-value storage engine.
- **Component**: Storage Engine (`byoridb-kvstore` embeds the C++ library)
- **License**: Apache 2.0 (Dual Licensed with GPLv2; we elect Apache 2.0)
- **Copyright**: Copyright (c) 2011-present, Facebook, Inc. All rights reserved.
- **Source**: https://github.com/facebook/rocksdb

## Rust Crates
This project uses various Rust crates from crates.io. Key dependencies include:

| Crate | License | Usage |
|-------|---------|-------|
| **tokio** | MIT | Asynchronous Runtime |
| **tonic** | MIT | gRPC Framework |
| **prost** | Apache 2.0 | Protocol Buffers implementation |
| **serde** | MIT/Apache 2.0 | Serialization framework |
| **rocksdb** | MIT/Apache 2.0 | Rust bindings for RocksDB |
| **clap** | MIT/Apache 2.0 | Command Line Argument Parser |
| **rustyline** | MIT | Readline implementation for CLI |
| **comfy-table** | MIT | CLI Table Formatting |
| **anyhow** | MIT/Apache 2.0 | Error handling |
| **thiserror** | MIT/Apache 2.0 | Error handling |
| **tracing** | MIT | Structured logging |

For a complete list of dependencies, please refer to the `Cargo.toml` and `Cargo.lock` files.

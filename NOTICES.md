# Open Source Software Notices

This project is built using the following open source software:

## redb
ByoriDB uses redb as its underlying key-value storage engine.
- **Component**: Storage Engine (`byoridb-kvstore`, pure-Rust embedded KV)
- **License**: MIT/Apache 2.0
- **Source**: https://github.com/cberner/redb

## Rust Crates
This project uses various Rust crates from crates.io. Key dependencies include:

| Crate | License | Usage |
|-------|---------|-------|
| **tokio** | MIT | Asynchronous Runtime |
| **tonic** | MIT | gRPC Framework |
| **prost** | Apache 2.0 | Protocol Buffers implementation |
| **serde** | MIT/Apache 2.0 | Serialization framework |
| **redb** | MIT/Apache 2.0 | Pure-Rust embedded key-value store |
| **clap** | MIT/Apache 2.0 | Command Line Argument Parser |
| **rustyline** | MIT | Readline implementation for CLI |
| **comfy-table** | MIT | CLI Table Formatting |
| **anyhow** | MIT/Apache 2.0 | Error handling |
| **thiserror** | MIT/Apache 2.0 | Error handling |
| **tracing** | MIT | Structured logging |

For a complete list of dependencies, please refer to the `Cargo.toml` and `Cargo.lock` files.

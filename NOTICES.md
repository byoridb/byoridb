# Open-source software notices

[한국어 비공식 번역](NOTICES.ko.md)

This English file is the project-maintained notices document. The Korean file is
an unofficial convenience translation. The [Apache License 2.0](LICENSE) and
each dependency's own license and notice files remain authoritative for their
respective software.

ByoriDB is Copyright 2024 ByoriDB contributors and is distributed under the
Apache License, Version 2.0.

## Third-party software

ByoriDB is built with open-source Rust crates. The following table identifies
key direct dependencies in the current workspace; it is not an exhaustive list
of direct and transitive packages.

| Component | License expression | Use |
|---|---|---|
| [redb](https://github.com/cberner/redb) | MIT OR Apache-2.0 | Embedded key-value storage |
| [Tokio](https://github.com/tokio-rs/tokio) | MIT | Asynchronous runtime |
| [tonic](https://github.com/hyperium/tonic) | MIT | gRPC framework |
| [Prost](https://github.com/tokio-rs/prost) | Apache-2.0 | Protocol Buffers implementation |
| [Axum](https://github.com/tokio-rs/axum) | MIT | HTTP server framework |
| [Serde](https://github.com/serde-rs/serde) | MIT OR Apache-2.0 | Serialization |
| [Argon2](https://github.com/RustCrypto/password-hashes) | MIT OR Apache-2.0 | Password hashing |
| [Clap](https://github.com/clap-rs/clap) | MIT OR Apache-2.0 | Command-line parsing |
| [rustyline](https://github.com/kkawakam/rustyline) | MIT | Interactive CLI line editing |
| [comfy-table](https://github.com/Nukesor/comfy-table) | MIT | CLI table rendering |
| [instant-distance](https://github.com/InstantDomain/instant-distance) | MIT OR Apache-2.0 | HNSW vector indexing |
| [metrics](https://github.com/metrics-rs/metrics) | MIT | Metrics facade |
| [metrics-exporter-prometheus](https://github.com/metrics-rs/metrics) | MIT | Prometheus metrics export |
| [tracing](https://github.com/tokio-rs/tracing) | MIT | Structured diagnostics |
| [anyhow](https://github.com/dtolnay/anyhow) | MIT OR Apache-2.0 | Application error handling |
| [thiserror](https://github.com/dtolnay/thiserror) | MIT OR Apache-2.0 | Typed error definitions |
| [config-rs](https://github.com/rust-cli/config-rs) | MIT OR Apache-2.0 | Configuration loading |

The exact resolved dependency graph is recorded in `Cargo.lock`; direct
dependency declarations are in the root and member `Cargo.toml` files. A
distributor should inspect the resolved versions and include all license texts,
copyright notices, and attribution required by those packages. This summary is
informational and does not replace that review.

## No endorsement

Names and links are provided only to identify upstream software. They do not
imply endorsement by the upstream projects, and the ByoriDB license does not
grant additional rights to their trademarks.

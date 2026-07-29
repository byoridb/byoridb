# 오픈소스 소프트웨어 고지

[English — authoritative project notices](NOTICES.md)

이 문서는 편의를 위한 비공식 한국어 번역입니다. 프로젝트가 관리하는 고지 문서는
영문 [NOTICES.md](NOTICES.md)이며, [Apache License 2.0](LICENSE)과 각 dependency의
license 및 notice 파일이 해당 software에 대한 최종 기준입니다. 번역과 영문 사이에
차이가 있으면 영문 및 upstream 원문을 따릅니다.

ByoriDB의 저작권은 Copyright 2024 ByoriDB contributors에 있으며 Apache License,
Version 2.0에 따라 배포됩니다.

## Third-party software

ByoriDB는 open-source Rust crate로 빌드됩니다. 다음 표는 현재 workspace의 주요 direct
dependency를 식별한 것이며 direct/transitive package 전체 목록은 아닙니다.

| Component | License expression | 용도 |
|---|---|---|
| [redb](https://github.com/cberner/redb) | MIT OR Apache-2.0 | Embedded key-value storage |
| [Tokio](https://github.com/tokio-rs/tokio) | MIT | Asynchronous runtime |
| [tonic](https://github.com/hyperium/tonic) | MIT | gRPC framework |
| [Prost](https://github.com/tokio-rs/prost) | Apache-2.0 | Protocol Buffers 구현 |
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
| [thiserror](https://github.com/dtolnay/thiserror) | MIT OR Apache-2.0 | Typed error 정의 |
| [config-rs](https://github.com/rust-cli/config-rs) | MIT OR Apache-2.0 | Configuration loading |

정확히 resolve된 dependency graph는 `Cargo.lock`에 기록되며 direct dependency 선언은
root와 member `Cargo.toml`에 있습니다. 배포자는 resolve된 version을 검토하고 각
package가 요구하는 모든 license text, copyright notice 및 attribution을 포함해야
합니다. 이 요약은 정보 제공 목적이며 해당 검토를 대체하지 않습니다.

## 보증 또는 지지 관계 없음

이름과 link는 upstream software를 식별하기 위해서만 제공합니다. Upstream project의
지지를 의미하지 않으며 ByoriDB license는 해당 trademark에 대한 추가 권리를
부여하지 않습니다.

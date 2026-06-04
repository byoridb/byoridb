// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // proto/gRPC code is only generated for the `distributed` feature; the
    // embedded (wasm-capable) build skips tonic-build entirely.
    if std::env::var_os("CARGO_FEATURE_DISTRIBUTED").is_none() {
        return Ok(());
    }
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&["proto/meta.proto"], &["proto"])?;

    tonic_build::configure()
        .build_server(false)
        .build_client(true)
        .compile_protos(&["proto/storage.proto"], &["proto"])?;
    Ok(())
}

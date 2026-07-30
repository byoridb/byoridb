fn main() -> Result<(), Box<dyn std::error::Error>> {
    // proto/gRPC code is only generated for the `distributed` feature; the
    // embedded (wasm-capable) build skips gRPC code generation entirely.
    if std::env::var_os("CARGO_FEATURE_DISTRIBUTED").is_some() {
        tonic_prost_build::compile_protos("proto/raft.proto")?;
        tonic_prost_build::compile_protos("proto/storage.proto")?;
    }
    Ok(())
}

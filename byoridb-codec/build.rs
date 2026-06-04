// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    prost_build::compile_protos(&["proto/data.proto"], &["proto"])?;
    Ok(())
}

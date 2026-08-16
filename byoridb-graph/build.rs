fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Declare the dependency explicitly. This crate's generated module must not
    // be reused after the schema changes: a stale copy compiles fine and then
    // disagrees with the server about the wire format.
    println!("cargo:rerun-if-changed=proto/graph.proto");
    tonic_prost_build::compile_protos("proto/graph.proto")?;
    Ok(())
}

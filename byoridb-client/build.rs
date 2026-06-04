fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::compile_protos("../byoridb-graph/proto/graph.proto")?;
    Ok(())
}

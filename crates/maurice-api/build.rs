fn main() {
    let proto_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../proto");
    let files = ["common", "workspaces", "services", "plans", "secrets"]
        .map(|f| format!("{proto_dir}/maurice/v1/{f}.proto"));
    for f in &files {
        println!("cargo:rerun-if-changed={f}");
    }
    connectrpc_build::Config::new()
        .files(&files)
        .includes(&[proto_dir])
        .include_file("_connectrpc.rs")
        .compile()
        .expect("compile proto");
}

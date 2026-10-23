fn main() {
    // rust-embed needs the UI output folder to exist even when the UI was never built.
    let dist = concat!(env!("CARGO_MANIFEST_DIR"), "/../../packages/web/dist");
    std::fs::create_dir_all(dist).expect("create packages/web/dist");
    let proto_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../proto");
    let files = [
        "common",
        "workspaces",
        "services",
        "plans",
        "secrets",
        "projects",
    ]
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

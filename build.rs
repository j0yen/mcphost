//! Build script: (1) generates the `connect::Provider` table from
//! `providers.toml` (PRD-mcphost-connection-provider-registry) -- a malformed
//! row fails the build naming the provider and the field; (2) stamps the git
//! sha (see `build_support/git_sha.rs`).

#[path = "src/connect/codegen.rs"]
mod codegen;
#[path = "build_support/git_sha.rs"]
mod git_sha;

fn generate_providers() {
    println!("cargo:rerun-if-changed=providers.toml");
    println!("cargo:rerun-if-changed=src/connect/codegen.rs");
    let text = std::fs::read_to_string("providers.toml").expect("read providers.toml");
    let rows = match codegen::parse_table(&text) {
        Ok(rows) => rows,
        Err(e) => panic!("{e}"),
    };
    let out = std::path::Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("providers_gen.rs");
    std::fs::write(out, codegen::render_rust(&rows)).expect("write providers_gen.rs");
}

fn main() {
    generate_providers();
    git_sha::main();
}

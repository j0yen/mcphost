//! `cargo xtask recipes [--check]` -- regenerate (or verify) every recipe's
//! llms.txt block, proof script, synthorg task yaml and golden-path test
//! from `recipes/*.toml`. Exit codes: 0 ok, 1 drift/IO, 2 a step names an
//! unknown tool or argument.

use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("recipes") => {
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
            let root = root.canonicalize().unwrap_or(root);
            std::process::exit(mcphost::recipes::cli(&root, &args[1..]));
        }
        _ => {
            eprintln!("usage: cargo xtask recipes [--check]");
            std::process::exit(64);
        }
    }
}

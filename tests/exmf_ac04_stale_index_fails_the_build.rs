//! AC4: with `index.json` older than an edited tool file, `cargo build`
//! fails with the stale message naming that tool file. Runs the real
//! `build.rs` over a scratch copy of `examples/`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().map(|e| e.unwrap()) {
        let dest = to.join(e.file_name());
        if e.path().is_dir() {
            copy_dir(&e.path(), &dest);
        } else {
            std::fs::copy(e.path(), dest).unwrap();
        }
    }
}

/// A scratch package that runs this tree's real `build.rs` over a copy of
/// `examples/`: same build-dependencies and lockfile, an empty library, so
/// the build script is the only thing that has to compile there (building
/// mcphost itself in a second location would recompile every dependency).
fn scratch(tag: &str) -> PathBuf {
    let tmp = std::env::temp_dir().join(format!("exmf-ac04-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("src")).unwrap();
    std::fs::write(tmp.join("src/lib.rs"), "").unwrap();
    std::fs::write(
        tmp.join("Cargo.toml"),
        "[package]\nname = \"scratch\"\nversion = \"0.0.0\"\nedition = \"2024\"\nbuild = \"build.rs\"\n\n[workspace]\n\n[build-dependencies]\nserde_json = \"1\"\nsha2 = \"0.10\"\ntoml = \"0.8\"\n",
    )
    .unwrap();
    std::fs::copy(root().join("Cargo.lock"), tmp.join("Cargo.lock")).unwrap();
    std::fs::copy(root().join("build.rs"), tmp.join("build.rs")).unwrap();
    std::fs::copy(root().join("src/doc_scan.rs"), tmp.join("src/doc_scan.rs")).unwrap();
    std::fs::copy(root().join("rust-toolchain.toml"), tmp.join("rust-toolchain.toml")).unwrap();
    std::fs::copy(root().join("providers.toml"), tmp.join("providers.toml")).unwrap();
    copy_dir(&root().join("examples"), &tmp.join("examples"));
    tmp
}

fn cargo_build(tmp: &Path) -> std::process::Output {
    // test exe lives in <target>/debug/deps/; a sibling dir keeps the scratch
    // package from touching the main build's lock and artifacts.
    let target = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().parent().unwrap().to_path_buf();
    Command::new(option_env!("CARGO").unwrap_or("cargo"))
        .args(["build", "--offline", "--manifest-path"])
        .arg(tmp.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", target.join("exmf-scratch"))
        .env_remove("MCPHOST_EXAMPLES_REGEN")
        .output()
        .expect("run cargo build")
}

#[test]
fn an_edited_tool_file_makes_the_build_fail_naming_it() {
    let tmp = scratch("edit");
    let tool = tmp.join("examples/team-memory/tools/recall.py");
    let mut src = std::fs::read_to_string(&tool).unwrap();
    src.push_str("\n# edited after index.json was generated\n");
    std::fs::write(&tool, src).unwrap();
    let out = cargo_build(&tmp);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "build unexpectedly succeeded");
    assert!(stderr.contains("examples/index.json is stale; run mcphost examples index"), "stderr: {stderr}");
    assert!(stderr.contains("examples/team-memory/tools/recall.py"), "stderr: {stderr}");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn an_unindexed_directory_makes_the_build_fail_naming_it() {
    let tmp = scratch("dir");
    std::fs::create_dir(tmp.join("examples/brand-new")).unwrap();
    let out = cargo_build(&tmp);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "build unexpectedly succeeded");
    assert!(stderr.contains("examples/index.json is stale"), "stderr: {stderr}");
    assert!(stderr.contains("examples/brand-new"), "stderr: {stderr}");
    let _ = std::fs::remove_dir_all(&tmp);
}

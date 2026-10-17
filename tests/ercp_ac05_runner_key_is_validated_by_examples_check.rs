//! PRD-mcphost-example-recipes-conform-to-policy
//! AC5 — Given `examples/webhook-inbox/example.toml` with `runner:
//! orch-only`, When `mcphost examples check` runs, Then it passes, and a
//! value outside `{any, orch-only}` fails naming the file.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn tree(tag: &str, runner_line: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("ercp-ac05-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let dir = root.join("demo");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("proof.sh"), "#!/bin/sh\n").unwrap();
    std::fs::write(
        dir.join("example.toml"),
        format!(
            "[example]\nname = \"demo\"\ntitle = \"Demo\"\nsummary = \"A demo.\"\nuse_case = [\"chart\"]\nclients = [\"any\"]\nsince = \"0.1.0\"\n{runner_line}\n\n[first_call]\ntool = \"host.ping\"\nexpect = \"result.a == 1\"\n"
        ),
    )
    .unwrap();
    root
}

fn check(dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["examples", "check", "--dir"])
        .arg(dir)
        .output()
        .expect("run mcphost examples check")
}

#[test]
fn ercp_ac05() {
    // The shipped tree: webhook-inbox declares orch-only and the whole tree passes.
    let manifest = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/webhook-inbox/example.toml")).unwrap();
    assert!(manifest.lines().any(|l| l.trim() == "runner = \"orch-only\""), "webhook-inbox must declare runner = \"orch-only\"");
    let out = check(&Path::new(env!("CARGO_MANIFEST_DIR")).join("examples"));
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));

    // Both accepted values pass.
    for ok in ["runner = \"any\"", "runner = \"orch-only\""] {
        let root = tree("ok", ok);
        let out = check(&root);
        assert_eq!(out.status.code(), Some(0), "{ok}: {}", String::from_utf8_lossy(&out.stderr));
        let _ = std::fs::remove_dir_all(&root);
    }

    // A value outside the set fails, naming the file.
    let root = tree("bad", "runner = \"host-only\"");
    let out = check(&root);
    assert_eq!(out.status.code(), Some(2), "stdout: {}", String::from_utf8_lossy(&out.stdout));
    let stderr = String::from_utf8_lossy(&out.stderr);
    for needle in ["examples/demo/example.toml", "example.runner", "host-only"] {
        assert!(stderr.contains(needle), "stderr missing `{needle}`: {stderr}");
    }
    let _ = std::fs::remove_dir_all(&root);
}

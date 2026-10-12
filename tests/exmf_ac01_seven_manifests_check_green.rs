//! AC1: every existing example directory has a manifest that fits R1, and
//! `mcphost examples check` exits 0 reporting that many valid entries. A
//! directory that cannot be described without a new field is the PRD's
//! falsifier firing and is reported by name.

use std::path::Path;
use std::process::Command;

fn examples_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples")
}

#[test]
fn every_example_directory_has_a_manifest_and_check_exits_zero() {
    let dirs = mcphost::examples::example_dirs(&examples_dir()).expect("read examples/");
    assert!(!dirs.is_empty(), "examples/ has no directories");
    let kinds = mcphost::examples::registered_kind_names();
    let mut falsifier = Vec::new();
    for d in &dirs {
        if let Err(errs) = mcphost::examples::load_one(d, &kinds) {
            for e in errs {
                falsifier.push(format!("FALSIFIER FIRED for `{}`: {e}", e.dir));
            }
        }
    }
    assert!(falsifier.is_empty(), "{}", falsifier.join("\n"));

    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["examples", "check", "--dir"])
        .arg(examples_dir())
        .output()
        .expect("run mcphost examples check");
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(&format!("{} examples", dirs.len())), "stdout: {stdout}");
}

#[test]
fn a_field_outside_r1_is_rejected() {
    let tmp = std::env::temp_dir().join(format!("exmf-ac01-{}", std::process::id()));
    let dir = tmp.join("extra-field");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("proof.sh"), "#!/bin/sh\n").unwrap();
    std::fs::write(
        dir.join("example.toml"),
        "[example]\nname = \"extra-field\"\ntitle = \"t\"\nsummary = \"s\"\nuse_case = [\"chart\"]\nclients = [\"any\"]\nsince = \"0.1.0\"\ninvented = 1\n\n[first_call]\ntool = \"x\"\nexpect = \"result.a == 1\"\n",
    )
    .unwrap();
    let kinds = mcphost::examples::registered_kind_names();
    let errs = mcphost::examples::collect(&tmp, &kinds).unwrap_err();
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].dir, "extra-field");
    assert!(errs[0].reason.contains("invented"), "{}", errs[0]);
    let _ = std::fs::remove_dir_all(&tmp);
}

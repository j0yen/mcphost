//! PRD-mcphost-contract-suite-markers AC7 -- given one unmarked reader of a
//! derived artifact, `scripts/contract-lint.sh --fix <dir>` prints exactly one
//! line `<file>: add  //! contract: reads <artifact>` and leaves the file
//! unchanged.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn ctrm_ac07_fix_prints_one_line_and_leaves_the_file_unchanged() {
    // Built from pieces so this file's own source never names the artifact.
    let artifact = format!("{}/{}", "agent", "intent-card.json");
    let dir = std::env::temp_dir().join(format!("ctrm-ac07-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let body = format!(
        "//! just a test\nfn f() {{ let _ = std::path::Path::new(env!(\"CARGO_MANIFEST_DIR\")).join(\"agent\").join(\"{}\"); }}\n#[test]\nfn t() {{}}\n",
        "intent-card.json"
    );
    let file = dir.join("zz_reader.rs");
    fs::write(&file, &body).unwrap();

    let out = Command::new(root().join("scripts/contract-lint.sh"))
        .arg("--fix")
        .arg(&dir)
        .current_dir(root())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "{stdout}");
    assert_eq!(lines[0], format!("{}: add  //! contract: reads {artifact}", file.display()));
    assert_eq!(fs::read_to_string(&file).unwrap(), body);
    let _ = fs::remove_dir_all(&dir);
}

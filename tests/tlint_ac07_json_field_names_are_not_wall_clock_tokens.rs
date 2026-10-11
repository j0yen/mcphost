//! PRD-mcphost-timing-assertion-lint
//! AC7 (P0) — Given a scratch file asserting `body["p95_ms"].is_number()`,
//! When the script runs, Then exit 0 (a JSON field name is not a wall-clock
//! token).

use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn json_field_named_p95_ms_is_not_a_wall_clock_token() {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-tlint-ac07-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    fs::create_dir_all(dir.join("tests")).expect("mkdir scratch tests/");
    fs::write(
        dir.join("tests/usage_metering.rs"),
        "#[test]\nfn fields() {\n    assert!(body[\"p95_ms\"].is_number());\n    assert!(body[\"p50_ms\"].is_number(), \"elapsed() in a message\");\n}\n",
    )
    .expect("write fixture");
    let out = Command::new("python3")
        .arg("-I")
        .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/timing-lint.py"))
        .arg("tests")
        .current_dir(&dir)
        .output()
        .expect("run scripts/timing-lint.py");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "stdout:\n{stdout}");
    assert!(stdout.contains("timing-lint: 0 offenders"), "{stdout}");
}

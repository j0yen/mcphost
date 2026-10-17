//! PRD-mcphost-launch-check-generated-b AC2 (P0) -- Given an unreadable or
//! malformed TOML, When `launch-check` runs, Then it exits 2 with the parse
//! error and line, and prints no `green` line.

use std::process::Command;

#[test]
fn lchkb_ac02_malformed_toml_exits_two_with_line_and_no_green() {
    let dir = std::env::temp_dir().join(format!("lchkb-ac02-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let toml = dir.join("items.toml");
    std::fs::write(&toml, "[[item]]\nid = \"a\"\nwave = 1\nowner = \"host\"\nprobe = \"echo ok\"\nexpect = \n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["launch-check", "--items"])
        .arg(&toml)
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("line 6"), "{stderr}");
    assert!(!String::from_utf8_lossy(&out.stdout).contains("green"));
}

#[test]
fn lchkb_ac02_unreadable_toml_exits_two_with_path_and_no_green() {
    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["launch-check", "--items", "/nonexistent/items.toml"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("/nonexistent/items.toml"));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("green"));
}

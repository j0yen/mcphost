//! PRD-mcphost-launch-check-generated AC2 (P0) -- Given a TOML with one probe
//! that exits 0 and prints `ok`, one that prints `nope` against
//! `expect = "ok"`, one that sleeps past `timeout_s = 1`, and one `joe` row,
//! When `launch-check` runs, Then it prints `green`, `red … (expected "ok")`,
//! `red … (timeout 1s)`, `manual …`, summary `1/3 green`, and exits 0.

use std::process::Command;

const FOUR_ROWS: &str = r#"
[[item]]
id = "p-ok"
wave = 1
owner = "host"
probe = "echo ok"
expect = "ok"
doc = "prints ok"

[[item]]
id = "p-nope"
wave = 1
owner = "host"
probe = "echo nope"
expect = "ok"
doc = "prints nope"

[[item]]
id = "p-slow"
wave = 1
owner = "host"
probe = "sleep 30"
expect = "exit0"
timeout_s = 1
doc = "sleeps"

[[item]]
id = "j-submit"
wave = 1
owner = "joe"
doc = "Click submit."
url = "https://example.com/submit"
"#;

#[test]
fn lchk_ac02_prints_green_red_timeout_manual_and_summary_and_exits_zero() {
    let dir = std::env::temp_dir().join(format!("lchk-ac02-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let toml = dir.join("items.toml");
    std::fs::write(&toml, FOUR_ROWS).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .arg("launch-check")
        .arg("--items")
        .arg(&toml)
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("green p-ok\n"), "{text}");
    assert!(text.contains("red p-nope (expected \"ok\")\n"), "{text}");
    assert!(text.contains("red p-slow (timeout 1s)\n"), "{text}");
    assert!(text.contains("manual j-submit: Click submit.\n"), "{text}");
    assert!(text.contains("1/3 green\n"), "{text}");
}

#[test]
fn lchk_ac02_unreadable_toml_exits_two() {
    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["launch-check", "--items", "/nonexistent/items.toml"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

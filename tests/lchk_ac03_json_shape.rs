//! PRD-mcphost-launch-check-generated AC3 (P0) -- Given `--json`, When run on
//! the same TOML, Then the output parses, `items` has 4 entries with
//! `status` in {green, red, manual}, `green == 1`, `total == 3`.

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
fn lchk_ac03_json_has_four_items_one_green_three_total() {
    let dir = std::env::temp_dir().join(format!("lchk-ac03-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let toml = dir.join("items.toml");
    std::fs::write(&toml, FOUR_ROWS).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["launch-check", "--json", "--items"])
        .arg(&toml)
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
    let items = v["items"].as_array().expect("items array");
    assert_eq!(items.len(), 4);
    for it in items {
        let st = it["status"].as_str().unwrap();
        assert!(["green", "red", "manual"].contains(&st), "{st}");
        for k in ["id", "owner", "reason", "duration_ms"] {
            assert!(it.get(k).is_some(), "missing {k}");
        }
    }
    assert_eq!(v["green"], 1);
    assert_eq!(v["total"], 3);
    assert!(v.get("wave").is_some() && v["checked_at"].as_str().is_some_and(|s| s.ends_with('Z')));
}

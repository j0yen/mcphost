//! PRD-mcphost-launch-check-generated-b AC3 (P1) -- Given `--out <dir>`, When
//! run twice on the same UTC day, Then `<dir>/launch-check-<date>.json` holds
//! the latest run and the previous is kept as `…-<date>.1.json`.

use std::process::Command;

fn run(dir: &std::path::Path, out: &std::path::Path, id: &str) {
    let toml = dir.join("items.toml");
    std::fs::write(
        &toml,
        format!("[[item]]\nid = \"{id}\"\nwave = 1\nowner = \"host\"\nprobe = \"echo ok\"\nexpect = \"ok\"\ndoc = \"d\"\n"),
    )
    .unwrap();
    let st = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["launch-check", "--json", "--items"])
        .arg(&toml)
        .arg("--out")
        .arg(out)
        .current_dir(dir)
        .output()
        .unwrap();
    assert_eq!(st.status.code(), Some(0), "{}", String::from_utf8_lossy(&st.stderr));
}

#[test]
fn lchkb_ac03_second_run_same_day_rotates_previous_to_dot_one() {
    let dir = std::env::temp_dir().join(format!("lchkb-ac03-{}", std::process::id()));
    let out = dir.join("nightly");
    std::fs::create_dir_all(&dir).unwrap();
    run(&dir, &out, "first-run");
    run(&dir, &out, "second-run");
    let mut names: Vec<String> =
        std::fs::read_dir(&out).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names.len(), 2, "{names:?}");
    let latest = names.iter().find(|n| !n.ends_with(".1.json")).unwrap();
    let prev = names.iter().find(|n| n.ends_with(".1.json")).unwrap();
    assert!(latest.starts_with("launch-check-") && latest.ends_with(".json"), "{latest}");
    assert_eq!(prev.trim_end_matches(".1.json"), latest.trim_end_matches(".json"), "{names:?}");
    let latest_body = std::fs::read_to_string(out.join(latest)).unwrap();
    let prev_body = std::fs::read_to_string(out.join(prev)).unwrap();
    assert!(latest_body.contains("second-run") && !latest_body.contains("first-run"), "{latest_body}");
    assert!(prev_body.contains("first-run"), "{prev_body}");
    let v: serde_json::Value = serde_json::from_str(&latest_body).unwrap();
    assert_eq!(v["green"], 1);
}

//! PRD-mcphost-sse-frame-contract-generated AC7 (P1) -- Given `www/llms.txt`,
//! When the drift check runs, Then its Transport section equals the script's
//! rendering byte for byte.

use std::process::Command;

#[test]
fn transport_section_equals_the_scripts_rendering_and_is_at_most_six_lines() {
    let root = env!("CARGO_MANIFEST_DIR");
    let rendered = Command::new("bash")
        .arg(format!("{root}/scripts/protocol-frames-check.sh"))
        .arg("--render-llms")
        .output()
        .expect("run --render-llms");
    assert!(rendered.status.success(), "{}", String::from_utf8_lossy(&rendered.stderr));
    let rendered = String::from_utf8(rendered.stdout).expect("utf-8");

    let llms = std::fs::read_to_string(format!("{root}/www/llms.txt")).expect("read www/llms.txt");
    let start = llms.find("<!-- frames:start -->").expect("frames:start marker in www/llms.txt");
    let end_marker = "<!-- frames:end -->";
    let end = llms.find(end_marker).expect("frames:end marker in www/llms.txt") + end_marker.len();
    let section = format!("{}\n", &llms[start..end]);
    assert_eq!(section, rendered, "www/llms.txt Transport section drifted; run scripts/protocol-frames-check.sh --write");
    assert!(section.contains("\n## Transport\n"), "the block must carry the Transport heading");
    assert!(section.lines().count() <= 6, "Transport section is at most 6 lines, got {}", section.lines().count());

    let check = Command::new("bash").arg(format!("{root}/scripts/protocol-frames-check.sh")).arg("--check").output().expect("run --check");
    assert!(check.status.success(), "{}", String::from_utf8_lossy(&check.stderr));
}

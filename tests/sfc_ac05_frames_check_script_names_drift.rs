//! PRD-mcphost-sse-frame-contract-generated AC5 (P0) -- Given
//! `scripts/protocol-frames-check.sh --check` after a row is added to the
//! table without `--write`, When it runs, Then it exits 1 naming
//! `docs/protocol/frames.md` and `www/frames.json`; after `--write`,
//! exit 0.
//!
//! Runs against a scratch copy of the five files the script reads and writes,
//! so the committed tree is never touched.

use std::path::Path;
use std::process::{Command, Output};

const FILES: &[&str] = &[
    "scripts/protocol-frames-check.sh",
    "src/protocol/frames.rs",
    "docs/protocol/frames.md",
    "www/frames.json",
    "www/llms.txt",
];

fn script(root: &Path, mode: &str) -> Output {
    Command::new("bash").arg(root.join("scripts/protocol-frames-check.sh")).arg(mode).output().expect("run script")
}

#[test]
fn a_new_row_without_write_exits_one_naming_both_files_and_write_fixes_it() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch = std::env::temp_dir().join(format!("sfc_ac05_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    for rel in FILES {
        let to = scratch.join(rel);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(repo.join(rel), &to).unwrap_or_else(|e| panic!("copy {rel}: {e}"));
    }

    let clean = script(&scratch, "--check");
    assert!(clean.status.success(), "committed tree must be in sync: {}", String::from_utf8_lossy(&clean.stderr));

    let frames_rs = scratch.join("src/protocol/frames.rs");
    let source = std::fs::read_to_string(&frames_rs).unwrap();
    let marker = "pub const FRAME_SEQUENCES: &[FrameRow] = &[\n";
    assert!(source.contains(marker), "table opener moved; update this test");
    let row = "    FrameRow {\n        request_type: \"tools/call:with_progress_token\",\n        frames: &[(FrameKind::Progress, Card::ZeroOrMore), (FrameKind::Result, Card::ExactlyOne)],\n        content_type_rule: ContentTypeRule::JsonWhenSingleFrame,\n    },\n";
    std::fs::write(&frames_rs, source.replacen(marker, &format!("{marker}{row}"), 1)).unwrap();

    let drifted = script(&scratch, "--check");
    let stderr = String::from_utf8_lossy(&drifted.stderr);
    assert_eq!(drifted.status.code(), Some(1), "drift must exit 1: {stderr}");
    assert!(stderr.contains("docs/protocol/frames.md"), "must name the doc: {stderr}");
    assert!(stderr.contains("www/frames.json"), "must name the json: {stderr}");

    let written = script(&scratch, "--write");
    assert!(written.status.success(), "--write failed: {}", String::from_utf8_lossy(&written.stderr));
    let after = script(&scratch, "--check");
    assert!(after.status.success(), "--check after --write must exit 0: {}", String::from_utf8_lossy(&after.stderr));
    let json = std::fs::read_to_string(scratch.join("www/frames.json")).unwrap();
    assert!(json.contains("tools/call:with_progress_token"), "the new row must reach frames.json");

    let _ = std::fs::remove_dir_all(&scratch);
}

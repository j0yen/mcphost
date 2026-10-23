//! PRD-mcphost-tools-call-progress-notifications
//! AC2 (P0) — Given `enum Frame` with `kind()` and the generated
//! `FrameKind::ALL`, When the frames tests run, Then every notification
//! variant of `FrameKind` has a `FRAME_SEQUENCES` row, `emit` matches `Frame`
//! exhaustively with no missing-sender arm, the row
//! `tools/call:with_progress_token → [progress {0,n}, result]` carries
//! `when: request _meta.progressToken present`, and
//! `scripts/protocol-frames-check.sh` renders frames.json, frames.md and
//! llms.txt byte-identical to the committed files.

use mcphost::protocol::frames::{FRAME_SEQUENCES, Frame, FrameKind, REQ_WITH_PROGRESS_TOKEN, row_for};
use std::path::Path;

const REPO: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn tcpn_ac02_every_notification_kind_has_a_row() {
    let notifications: Vec<FrameKind> = FrameKind::ALL.iter().copied().filter(|k| k.is_notification()).collect();
    assert!(notifications.contains(&FrameKind::Progress), "Progress is a notification kind: {notifications:?}");
    for kind in notifications {
        assert!(
            FRAME_SEQUENCES.iter().any(|row| row.frames.iter().any(|(k, _)| *k == kind)),
            "no FRAME_SEQUENCES row carries {kind:?} ({})",
            kind.method()
        );
    }
}

#[test]
fn tcpn_ac02_frame_kind_maps_every_variant() {
    use rmcp::model::{NumberOrString, ProgressNotificationParam, ProgressToken};
    let progress = Frame::Progress(ProgressNotificationParam::new(ProgressToken(NumberOrString::Number(1)), 1.0));
    assert_eq!(Frame::ToolListChanged.kind(), FrameKind::ListChanged);
    assert_eq!(Frame::PromptListChanged.kind(), FrameKind::PromptsListChanged);
    assert_eq!(progress.kind(), FrameKind::Progress);
}

#[test]
fn tcpn_ac02_emit_matches_frame_exhaustively() {
    let source = std::fs::read_to_string(Path::new(REPO).join("src/protocol/frames.rs")).expect("read frames.rs");
    let start = source.find("pub async fn emit(").expect("emit");
    let body = &source[start..];
    let body = &body[..body.find("\n}\n").expect("end of emit")];
    for arm in ["Frame::ToolListChanged =>", "Frame::PromptListChanged =>", "Frame::Progress(", ] {
        assert!(body.contains(arm), "emit has no `{arm}` sender arm:\n{body}");
    }
    for wildcard in ["other =>", "_ =>"] {
        assert!(!body.contains(wildcard), "emit must match Frame exhaustively, found `{wildcard}`:\n{body}");
    }
}

#[test]
fn tcpn_ac02_progress_row_carries_its_when_condition() {
    let row = row_for(REQ_WITH_PROGRESS_TOKEN).expect("tools/call:with_progress_token row");
    assert_eq!(REQ_WITH_PROGRESS_TOKEN, "tools/call:with_progress_token");
    let kinds: Vec<FrameKind> = row.frames.iter().map(|(k, _)| *k).collect();
    assert_eq!(kinds, [FrameKind::Progress, FrameKind::Result]);
    assert_eq!(row.frames[0].1.notation(), "0,n");
    assert_eq!(row.when, Some("request _meta.progressToken present"));
    let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(Path::new(REPO).join("www/frames.json")).unwrap()).unwrap();
    let served = json["rows"].as_array().unwrap().iter().find(|r| r["request_type"] == REQ_WITH_PROGRESS_TOKEN).expect("served row");
    assert_eq!(served["when"], "request _meta.progressToken present");
}

#[test]
fn tcpn_ac02_rendered_files_are_byte_identical_to_the_generator() {
    let out = std::process::Command::new("bash")
        .arg(Path::new(REPO).join("scripts/protocol-frames-check.sh"))
        .arg("--check")
        .current_dir(REPO)
        .output()
        .expect("run protocol-frames-check.sh");
    assert!(out.status.success(), "frames.json / frames.md / llms.txt drifted: {}", String::from_utf8_lossy(&out.stderr));
}

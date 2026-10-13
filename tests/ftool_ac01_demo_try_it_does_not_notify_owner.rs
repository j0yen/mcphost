//! PRD-mcphost-form-tool
//! AC1 (P0) — Given PRD-mcphost-demo-url-per-tool's ACs and
//! `src/public_tool.rs` `x_post`, When a test greps both for an owner-inbox
//! write on try-it submission, Then it records
//! `demo_try_it_notifies_owner: false` in its output (true parks this PRD).

use std::path::PathBuf;

const INBOX_MARKERS: &[&str] = &["msg_send", "messaging::", "msg.send", "inbox", "form_submission"];

fn mentions_inbox_write(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    INBOX_MARKERS.iter().any(|m| lower.contains(&m.to_ascii_lowercase()))
}

/// The demo PRD's ACs section, read from wherever the PRD lives (repo root
/// while queued/building, `built-prds` once archived). `None` when no copy is
/// reachable from this machine.
fn demo_prd_acs() -> Option<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let name = "PRD-mcphost-demo-url-per-tool.md";
    let candidates = [
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(name),
        PathBuf::from(&home).join("Documents/PRDs/built-prds").join(name),
        PathBuf::from(&home).join("Documents/PRDs/build-queue").join(name),
    ];
    let text = candidates.iter().find_map(|p| std::fs::read_to_string(p).ok())?;
    let acs = text.split("## Acceptance criteria").nth(1)?.to_string();
    Some(acs)
}

/// Source of the `x_post` handler: from its signature to the next top-level
/// item, which is where a try-it submission lands.
fn x_post_source() -> String {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/public_tool.rs")).expect("read public_tool.rs");
    let start = src.find("pub async fn x_post").expect("x_post exists");
    let rest = &src[start..];
    let end = rest.find("\n}\n").map(|i| i + 3).unwrap_or(rest.len());
    rest[..end].to_string()
}

#[test]
fn demo_try_it_notifies_owner_is_false() {
    let x_post = x_post_source();
    let x_post_writes_inbox = mentions_inbox_write(&x_post);
    let demo_src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/demo_page.rs")).expect("read demo_page.rs");
    let demo_src_writes_inbox = mentions_inbox_write(&demo_src);
    let acs = demo_prd_acs();
    let acs_mention_inbox = acs.as_deref().map(mentions_inbox_write).unwrap_or(false);

    let demo_try_it_notifies_owner = x_post_writes_inbox || demo_src_writes_inbox || acs_mention_inbox;
    println!("demo_prd_acs_read: {}", acs.is_some());
    println!("demo_try_it_notifies_owner: {demo_try_it_notifies_owner}");
    assert!(!demo_try_it_notifies_owner, "demo try-it already notifies the owner: this PRD is a rename and parks");
}

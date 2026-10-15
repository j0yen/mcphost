//! AC7 — `docs/tools.md` and `www/llms.txt` document `already_offboarded`
//! for `host.self.offboard`, and `mcphost llms-txt --check` (simulated via
//! `clients::render_into`) stays green.

use std::path::Path;

fn read(rel: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(rel))
        .unwrap_or_else(|e| panic!("{rel}: {e}"))
}

#[test]
fn tools_md_returns_block_names_already_offboarded() {
    let tools_md = read("docs/tools.md");
    let start = tools_md
        .find("### `host.self.offboard`")
        .expect("docs/tools.md must have a host.self.offboard Returns section");
    let section = &tools_md[start..];
    let end = section[1..].find("\n### ").map_or(section.len(), |i| i + 1);
    let block = &section[..end];
    assert!(
        block.contains("already_offboarded"),
        "host.self.offboard Returns block in docs/tools.md must list `already_offboarded`:\n{block}"
    );
}

#[test]
fn llms_txt_names_already_offboarded_in_selfoffboard_section() {
    let llms = read("www/llms.txt");
    let start = llms
        .find("`host.self.offboard()`")
        .expect("www/llms.txt must have a host.self.offboard section");
    let section = &llms[start..];
    let end = section[1..].find("\n## ").map_or(section.len(), |i| i + 1);
    let block = &section[..end];
    assert!(
        block.contains("already_offboarded"),
        "www/llms.txt self-offboard section must mention `already_offboarded`:\n{block}"
    );
}

/// Simulates `mcphost llms-txt --check`: the committed `www/llms.txt` must
/// already carry the rendered clients block -- i.e., re-rendering it would
/// produce no change.
#[test]
fn llms_txt_clients_block_is_already_rendered() {
    let llms = read("www/llms.txt");
    let rendered =
        mcphost::clients::render_into(&llms, mcphost::clients::builtin()).expect("render_into must succeed");
    assert_eq!(
        rendered, llms,
        "www/llms.txt clients block is stale (run `mcphost llms-txt`)"
    );
}

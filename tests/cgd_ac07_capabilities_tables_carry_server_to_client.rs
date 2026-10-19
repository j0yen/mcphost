//! contract: reads scripts/sandbox-api-doc-check.sh
//! PRD-mcphost-capabilities-generated-from-dispatch
//! AC7 (P1) -- Given `www/llms.txt` and `docs/protocol/capabilities.md`, When
//! `scripts/sandbox-api-doc-check.sh` runs, Then it exits 0 only if both
//! `## Capabilities` tables carry the `server_to_client` column byte-for-byte
//! from `render_capabilities_section()`; `clive_ac03` asserts
//! `data.code == "capability_unsupported"` for the `{}` body (its own file).

use mcphost::llms_txt::{CAPABILITIES_SECTION_END, CAPABILITIES_SECTION_START, render_capabilities_section};

fn block_of(rel: &str) -> String {
    let text = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)).expect("read doc");
    let s = text.find(CAPABILITIES_SECTION_START).unwrap_or_else(|| panic!("{rel}: no capabilities start marker"));
    let e = text.find(CAPABILITIES_SECTION_END).unwrap_or_else(|| panic!("{rel}: no capabilities end marker")) + CAPABILITIES_SECTION_END.len();
    text[s..e].to_string()
}

#[test]
fn cgd_ac07_both_tables_carry_the_server_to_client_column_byte_for_byte() {
    let rendered = render_capabilities_section();
    assert!(rendered.contains("| server_to_client |"), "renderer lost the column:\n{rendered}");
    assert!(rendered.contains("`elicitation/create` |"), "elicitation row lacks its server_to_client cell:\n{rendered}");
    for rel in ["www/llms.txt", "docs/protocol/capabilities.md"] {
        assert_eq!(block_of(rel), rendered, "{rel} drifted from render_capabilities_section(); run scripts/sandbox-api-doc-check.sh --write");
    }
}

#[test]
fn cgd_ac07_doc_check_script_exits_0_on_the_in_sync_tree() {
    let _guard = crate::common::DOC_CHECK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = std::process::Command::new("bash")
        .arg("scripts/sandbox-api-doc-check.sh")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run doc check");
    assert!(out.status.success(), "doc check failed: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
}

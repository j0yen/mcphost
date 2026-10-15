//! AC5 (P0) -- PRD-mcphost-server-capabilities-from-registry. `www/llms.txt`
//! and `docs/protocol/capabilities.md` each carry a `## Capabilities`
//! section equal byte-for-byte to `render_capabilities_section()`;
//! `scripts/sandbox-api-doc-check.sh` exits 0 only then, and exits 1 naming
//! the file when one row is edited.

use crate::common;
use mcphost::llms_txt::{CAPABILITIES_SECTION_END, CAPABILITIES_SECTION_START, render_capabilities_section};
use std::path::Path;
use std::process::{Command, Output};

const FILES: [&str; 2] = ["www/llms.txt", "docs/protocol/capabilities.md"];

fn block_of(text: &str) -> &str {
    let start = text.find(CAPABILITIES_SECTION_START).expect("start marker");
    let end = text.find(CAPABILITIES_SECTION_END).expect("end marker") + CAPABILITIES_SECTION_END.len();
    &text[start..end]
}

fn run_check(dir: &Path) -> Output {
    Command::new("bash")
        .arg(dir.join("scripts/sandbox-api-doc-check.sh"))
        .current_dir(dir)
        .env("MCPHOST_BIN", env!("CARGO_BIN_EXE_mcphost"))
        .output()
        .expect("run sandbox-api-doc-check.sh")
}

#[test]
fn committed_sections_equal_the_rendered_section_byte_for_byte() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let expected = render_capabilities_section();
    for rel in FILES {
        let text = std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"));
        assert_eq!(block_of(&text), expected, "{rel} ## Capabilities section drifted from render_capabilities_section()");
        assert!(block_of(&text).contains("\n## Capabilities\n"), "{rel}");
    }
}

#[test]
fn check_script_passes_clean_and_fails_naming_the_file_when_a_row_is_edited() {
    let clean = common::doc_check_scratch("creg-ac05-clean");
    let out = run_check(&clean);
    assert!(
        out.status.success(),
        "clean tree must exit 0, stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::remove_dir_all(&clean).ok();

    for rel in FILES {
        let dir = common::doc_check_scratch("creg-ac05-edited");
        let path = dir.join(rel);
        let text = std::fs::read_to_string(&path).expect("read");
        let edited = text.replace("| `resources` | no |", "| `resources` | yes |");
        assert_ne!(edited, text, "the edit must change {rel}");
        std::fs::write(&path, edited).expect("write");
        let out = run_check(&dir);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{rel}: edited row must exit 1, stderr:\n{stderr}");
        assert!(stderr.contains(rel), "{rel}: stderr must name the file:\n{stderr}");
        std::fs::remove_dir_all(&dir).ok();
    }
}

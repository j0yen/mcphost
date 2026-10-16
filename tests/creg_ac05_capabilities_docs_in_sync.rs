//! AC5 (P0) -- PRD-mcphost-server-capabilities-from-registry. `www/llms.txt`
//! and `docs/protocol/capabilities.md` each carry a `## Capabilities`
//! section equal byte-for-byte to `render_capabilities_section()`;
//! `scripts/sandbox-api-doc-check.sh` exits 0 only then, and exits 1 naming
//! the file when one row is edited. `--write` never empties a doc: a bad
//! render (empty, cargo output, or a failing `--doc` call) exits non-zero and
//! leaves every target byte-identical (runs 847/830, 2026-10-15: the old
//! `mcphost_bin ... --doc > docs/protocol/capabilities.md` truncated the file
//! before a failing `cargo run`).

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

/// A stand-in `MCPHOST_BIN`: `$FAKE_MODE` picks what it does, `real` execs the
/// real binary for every call it does not sabotage.
fn fake_bin(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("fake-mcphost.sh");
    let script = format!(
        "#!/usr/bin/env bash\n\
         case \"$FAKE_MODE\" in\n\
           empty) exit 0 ;;\n\
           garbage) echo 'error[E0599]: no variant named DocsProtocolCapabilitiesCapabilities'; exit 0 ;;\n\
           doc_fails) case \"$*\" in *--doc*) exit 101 ;; esac ;;\n\
           doc_empty) case \"$*\" in *--doc*) exit 0 ;; esac ;;\n\
         esac\n\
         exec {} \"$@\"\n",
        env!("CARGO_BIN_EXE_mcphost")
    );
    std::fs::write(&path, script).expect("write fake bin");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod fake bin");
    }
    path
}

fn run_write(dir: &Path, mode: &str) -> Output {
    Command::new("bash")
        .arg(dir.join("scripts/sandbox-api-doc-check.sh"))
        .arg("--write")
        .current_dir(dir)
        .env("MCPHOST_BIN", fake_bin(dir))
        .env("FAKE_MODE", mode)
        .output()
        .expect("run sandbox-api-doc-check.sh --write")
}

#[test]
fn write_with_a_bad_render_exits_nonzero_and_leaves_every_target_untouched() {
    for mode in ["empty", "garbage", "doc_fails", "doc_empty"] {
        let dir = common::doc_check_scratch("creg-ac05-badrender");
        let before: Vec<Vec<u8>> = FILES.iter().map(|rel| std::fs::read(dir.join(rel)).expect("read")).collect();
        let out = run_write(&dir, mode);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{mode}: a bad render must exit non-zero, stderr:\n{stderr}");
        assert!(!stderr.contains("Permission denied"), "{mode}: the fake binary must be runnable, stderr:\n{stderr}");
        if mode != "doc_fails" {
            assert!(stderr.contains("writing nothing"), "{mode}: the refusal must say so, stderr:\n{stderr}");
        }
        for (rel, was) in FILES.iter().zip(&before) {
            let now = std::fs::read(dir.join(rel)).expect("read after");
            assert!(!now.is_empty(), "{mode}: {rel} must never be emptied");
            assert_eq!(&now, was, "{mode}: {rel} must be byte-identical after a refused write");
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[test]
fn write_with_a_good_render_keeps_the_headings_and_is_a_fixed_point() {
    let dir = common::doc_check_scratch("creg-ac05-goodwrite");
    let doc = dir.join("docs/protocol/capabilities.md");
    let before = std::fs::read_to_string(&doc).expect("read");
    let out = run_write(&dir, "real");
    assert!(out.status.success(), "write on a clean tree must exit 0, stderr:\n{}", String::from_utf8_lossy(&out.stderr));
    let after = std::fs::read_to_string(&doc).expect("read after");
    assert!(after.starts_with("# MCP capabilities\n"), "title heading kept:\n{after}");
    assert!(after.contains("\n## Capabilities\n"), "section heading kept:\n{after}");
    assert_eq!(after, before, "a clean tree is a fixed point of --write");
    std::fs::remove_dir_all(&dir).ok();
}

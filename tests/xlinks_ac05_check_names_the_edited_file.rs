//! AC5 (PRD-mcphost-docs-external-links-resolve) -- Given either file's
//! block edited by hand, When `mcphost llms-txt --check` runs, Then it
//! exits 1 naming the file.

use crate::xlinks_cli;

#[test]
fn check_is_green_on_the_committed_files() {
    let s = xlinks_cli::Scratch::new("ac05-green");
    let out = s.llms_txt(true);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn a_hand_edit_to_either_block_exits_one_naming_that_file_only() {
    for (edited, other) in [("README.md", "llms.txt"), ("llms.txt", "README.md")] {
        let s = xlinks_cli::Scratch::new("ac05");
        let doc = s.read(edited);
        let hand_edited = doc.replacen("Docs: <", "Docs (hand edit): <", 1);
        assert_ne!(hand_edited, doc, "fixture edit must change the block");
        s.write(edited, &hand_edited);

        let out = s.llms_txt(true);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{edited}: {stderr}");
        assert!(stderr.contains(&s.path(edited).display().to_string()), "{edited} not named: {stderr}");
        assert!(!stderr.contains(&s.path(other).display().to_string()), "{other} wrongly named: {stderr}");
        assert_eq!(s.read(edited), hand_edited, "--check must not write");
    }
}

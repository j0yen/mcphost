//! PRD-mcphost-emitted-doc-pointers-resolve-b
//! AC2 (P1) — Given `mcphost llms-txt`, When run, Then `www/llms.txt`
//! contains the `## Anchors` block and `llms-txt --check` fails when one
//! line is deleted.

use crate::xlinks_cli;
use std::path::Path;

const START: &str = "<!-- anchors:start -->";
const END: &str = "<!-- anchors:end -->";

fn anchor_lines(doc: &str) -> Vec<String> {
    let start = doc.find(START).expect("anchors:start marker");
    let end = doc.find(END).expect("anchors:end marker");
    doc[start..end].lines().filter_map(|l| l.strip_prefix("- anchor: ")).map(str::to_string).collect()
}

#[test]
fn edpb_ac02_run_renders_the_anchors_block_and_check_catches_a_deleted_line() {
    let s = xlinks_cli::Scratch::new("edpb-ac02");
    let committed = s.read("llms.txt");
    // Start without the block: running the generator must put it back.
    let start = committed.find(START).expect("committed file has the block");
    let end = committed.find(END).unwrap() + END.len() + 1;
    s.write("llms.txt", &format!("{}{}", &committed[..start], &committed[end..]));
    let out = s.llms_txt(false);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let doc = s.read("llms.txt");
    assert!(doc.contains("## Anchors"), "no ## Anchors heading");
    assert_eq!(doc, committed, "the committed www/llms.txt is stale: run `mcphost llms-txt`");
    let lines = anchor_lines(&doc);
    assert!(lines.iter().any(|l| l == "README.md#call-limits"), "{lines:?}");

    // Deleting one line makes --check fail, naming the file, without writing.
    let victim = "- anchor: README.md#call-limits\n";
    let edited = doc.replacen(victim, "", 1);
    assert_ne!(edited, doc);
    s.write("llms.txt", &edited);
    let check = s.llms_txt(true);
    assert_eq!(check.status.code(), Some(1), "{}", String::from_utf8_lossy(&check.stderr));
    assert!(String::from_utf8_lossy(&check.stderr).contains(&s.path("llms.txt").display().to_string()));
    assert_eq!(s.read("llms.txt"), edited, "--check must not write");
}

#[test]
fn edpb_ac02_block_lists_every_pointer_src_emits_and_each_resolves() {
    let doc = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("www/llms.txt")).unwrap();
    let listed = anchor_lines(&doc);
    let report = mcphost::doc_anchors::check_repo(Path::new(env!("CARGO_MANIFEST_DIR")));
    assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
    for p in &report.emitted {
        assert!(listed.contains(&p.anchor), "{}:{} emits {} but the block does not list it", p.file, p.line, p.anchor);
    }
}

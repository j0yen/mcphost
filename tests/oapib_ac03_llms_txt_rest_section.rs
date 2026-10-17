//! PRD-mcphost-openapi-for-rest-facade-b
//! AC3 (P1) -- Given `mcphost llms-txt`, When run, Then `www/llms.txt`
//! contains a `## REST façade` section between `<!-- rest:start -->` /
//! `<!-- rest:end -->` naming `/openapi.json` and `/docs`, and `llms_txt`
//! tool-name parity still passes.

use mcphost::llms_txt::{REST_SECTION_END, REST_SECTION_START, render_rest_section, splice_rest};
use std::process::Command;

fn rest_block(content: &str) -> &str {
    let start = content.find("<!-- rest:start -->").expect("rest:start marker -- run `mcphost llms-txt`");
    let end = content.find("<!-- rest:end -->").expect("rest:end marker -- run `mcphost llms-txt`");
    &content[start..end]
}

#[test]
fn oapib_ac03() {
    assert_eq!((REST_SECTION_START, REST_SECTION_END), ("<!-- rest:start -->", "<!-- rest:end -->"));

    // `mcphost llms-txt --check` (run from the repo root) finds the committed
    // file current, i.e. the generator's output is what is committed.
    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["llms-txt", "--check"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("mcphost llms-txt --check");
    assert!(out.status.success(), "llms-txt --check: {}", String::from_utf8_lossy(&out.stderr));

    let committed = include_str!("../www/llms.txt");
    let block = rest_block(committed);
    assert!(block.contains("## REST façade"), "{block}");
    assert!(block.contains("`/openapi.json`") && block.contains("`/docs`"), "{block}");
    assert_eq!(committed.matches("<!-- rest:start -->").count(), 1);

    // Splicing is idempotent, and replaces a stale block in place.
    let section = render_rest_section();
    assert_eq!(splice_rest(committed, &section), committed);
    let stale = committed.replace("rendered at `/docs`", "rendered elsewhere");
    assert_ne!(stale, committed);
    assert_eq!(splice_rest(&stale, &section), committed);
}

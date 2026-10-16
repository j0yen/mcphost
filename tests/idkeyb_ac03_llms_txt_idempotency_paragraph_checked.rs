//! PRD-mcphost-idempotency-key-on-call-b AC3 — Given `www/llms.txt`, When `mcphost llms-txt --check`
//! runs, Then the Idempotency paragraph matches the `const` strings and the check exits 0.

use crate::xlinks_cli;
use mcphost::idempotency::{ARG_KEY, CONFLICT_CODE, CONFLICT_HINT, META_KEY, TIMEOUT_HINT};

#[test]
fn idkeyb_ac03() {
    let s = xlinks_cli::Scratch::new("idkeyb-ac03");
    let doc = s.read("llms.txt");
    let start = doc.find("<!-- idempotency:start -->").expect("start marker");
    let end_marker = "<!-- idempotency:end -->";
    let end = doc.find(end_marker).expect("end marker") + end_marker.len();
    let para = &doc[start..end];
    for needle in [META_KEY, ARG_KEY, CONFLICT_CODE, CONFLICT_HINT, TIMEOUT_HINT, "24 h"] {
        assert!(para.contains(needle), "paragraph must carry `{needle}`: {para}");
    }

    let check = s.llms_txt(true);
    assert_eq!(check.status.code(), Some(0), "{}", String::from_utf8_lossy(&check.stderr));

    // Drift is caught: a paragraph that disagrees with the consts fails --check without being written.
    let drifted = doc.replace(CONFLICT_HINT, "use whatever key you like");
    s.write("llms.txt", &drifted);
    let stale = s.llms_txt(true);
    assert_eq!(stale.status.code(), Some(1), "{}", String::from_utf8_lossy(&stale.stderr));
    assert_eq!(s.read("llms.txt"), drifted, "--check must not write");
}

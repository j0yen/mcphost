//! PRD-mcphost-event-kinds-registry-generated-b AC1 — Given `www/llms.txt` with its `## Events`
//! section edited by hand, When `mcphost llms-txt --check` runs, Then it exits 1 naming the
//! section; Given the regenerated file, Then it exits 0.

use crate::xlinks_cli;

#[test]
fn evkrb_ac01() {
    let s = xlinks_cli::Scratch::new("evkrb-ac01");
    let doc = s.read("llms.txt");
    assert!(doc.contains("## Events"), "llms.txt must carry the events section");
    let ok = s.llms_txt(true);
    assert_eq!(ok.status.code(), Some(0), "{}", String::from_utf8_lossy(&ok.stderr));

    let drifted = doc.replacen("- run.done — ", "- run.finished — ", 1);
    assert_ne!(drifted, doc, "edit must change the file");
    s.write("llms.txt", &drifted);
    let stale = s.llms_txt(true);
    assert_eq!(stale.status.code(), Some(1), "{}", String::from_utf8_lossy(&stale.stderr));
    let err = String::from_utf8_lossy(&stale.stderr);
    assert!(err.contains("## Events"), "must name the section: {err}");

    let regen = s.llms_txt(false);
    assert_eq!(regen.status.code(), Some(0), "{}", String::from_utf8_lossy(&regen.stderr));
    let again = s.llms_txt(true);
    assert_eq!(again.status.code(), Some(0), "{}", String::from_utf8_lossy(&again.stderr));
}

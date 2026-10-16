//! PRD-mcphost-llms-full-generated-from-schemas AC6: a corpus entry for
//! `host.tool_publish` with an executed mark puts its body and
//! `executed: pass <date>` in that tool's section; with no corpus every
//! section reads `example: none` and the count section lists them all.

use mcphost::docs_examples::{Entry, Executed};
use mcphost::kinds::KindRegistry;
use mcphost::plans::PlanCatalog;

fn section<'a>(text: &'a str, name: &str) -> &'a str {
    let head = format!("### {name}\n");
    let start = text.find(&head).expect("section present");
    let rest = &text[start + head.len()..];
    &rest[..rest.find("\n### ").unwrap_or(rest.len())]
}

#[test]
fn llf_ac06() {
    let kinds = KindRegistry::with_builtin();
    let plans = PlanCatalog::default_catalog();
    let body = "host.tool_publish(name=\"hello\", kind=\"echo\", spec={})";
    let corpus = [Entry {
        id: "docs/x.md#1".into(),
        tag: "mcphost-call".into(),
        attrs: Default::default(),
        body: body.into(),
        expect: None,
        executed: Some(Executed { status: "pass".into(), date: "2026-10-14".into() }),
    }];
    let with = mcphost::llms_txt::render_tool_sections(&kinds, &plans, &corpus);
    let sec = section(&with, "host.tool_publish");
    assert!(sec.contains(body), "{sec}");
    assert!(sec.contains("executed: pass 2026-10-14"), "{sec}");
    assert!(!sec.contains("example: none"), "{sec}");
    assert!(!with.split("count: ").nth(1).unwrap().contains("- `host.tool_publish`\n"));

    let without = mcphost::llms_txt::render_tool_sections(&kinds, &plans, &[]);
    let names = mcphost::llms_txt::tenant_tool_names(&kinds);
    assert_eq!(without.matches("example: none\n").count(), names.len());
    let tail = &without[without.find(mcphost::llms_txt::NO_EXAMPLE_HEADING).unwrap()..];
    assert!(tail.contains(&format!("count: {}\n", names.len())), "{}", &tail[..80]);
    for n in &names {
        assert!(tail.contains(&format!("- `{n}`\n")), "{n} missing from the count section");
    }
}

//! PRD-mcphost-llms-full-generated-from-schemas AC2: `host.tool_call`'s
//! section has a `budget.max_tool_latency_ms` row whose constraint is the
//! per-plan ceiling from `ceilings_json()`; a ceiling no argument names
//! lands under `### plan ceilings`.

use mcphost::kinds::KindRegistry;
use mcphost::plans::PlanCatalog;

fn section<'a>(text: &'a str, name: &str) -> &'a str {
    let head = format!("### {name}\n");
    let start = text.find(&head).expect("section present");
    let rest = &text[start + head.len()..];
    let end = rest.find("\n### ").unwrap_or(rest.len());
    &rest[..end]
}

#[test]
fn llf_ac02() {
    let plans = PlanCatalog::default_catalog();
    let out = mcphost::llms_txt::render_tool_sections(&KindRegistry::with_builtin(), &plans, &[]);
    let sec = section(&out, "host.tool_call");
    let row = sec
        .lines()
        .find(|l| l.starts_with("| `budget.max_tool_latency_ms`"))
        .expect("budget.max_tool_latency_ms row");
    let cells: Vec<&str> = row.split('|').map(str::trim).collect();
    assert_eq!(cells[4], "≤ 30000 (free), ≤ 120000 (pro)", "row: {row}");
    for p in &plans.plans {
        let want = p.ceilings_json()["budget"]["max_tool_latency_ms"].as_i64().unwrap();
        assert!(cells[4].contains(&format!("≤ {want} ({})", p.name)), "row: {row}");
    }

    // A ceiling with no matching argument is listed, not dropped.
    let pc = out.find("### plan ceilings").expect("### plan ceilings section");
    let tail = &out[pc..];
    let flat = plans.plans[0].ceilings_flat();
    let unmatched = flat
        .iter()
        .map(|(n, _)| n)
        .find(|n| !out[..pc].contains(&format!("| `{n}` |")))
        .expect("some ceiling matches no argument in the default catalog");
    assert!(tail.contains(&format!("- `{unmatched}`")), "{unmatched} missing from plan ceilings");
}

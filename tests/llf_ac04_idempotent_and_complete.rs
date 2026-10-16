//! PRD-mcphost-llms-full-generated-from-schemas AC4: rendering twice is
//! byte-identical; every name in anonymous ∪ authenticated `tools/list` has
//! exactly one `### <name>` section and no section names a tool outside it.

use std::collections::BTreeMap;

use mcphost::kinds::KindRegistry;
use mcphost::plans::PlanCatalog;

#[test]
fn llf_ac04() {
    let kinds = KindRegistry::with_builtin();
    let plans = PlanCatalog::default_catalog();
    let a = mcphost::llms_txt::render_tool_sections(&kinds, &plans, &[]);
    let b = mcphost::llms_txt::render_tool_sections(&kinds, &plans, &[]);
    assert_eq!(a.as_bytes(), b.as_bytes(), "render is not deterministic");

    let mut live: Vec<String> = mcphost::handler::tenant_tool_descriptors(&kinds)
        .into_iter()
        .map(|t| t.name.to_string())
        .chain(mcphost::handler::anonymous_tool_names(&kinds))
        .collect();
    live.sort();
    live.dedup();

    let mut sections: BTreeMap<&str, usize> = BTreeMap::new();
    for line in a.lines() {
        if let Some(name) = line.strip_prefix("### ") {
            if name != "plan ceilings" {
                *sections.entry(name).or_default() += 1;
            }
        }
    }
    for name in &live {
        assert_eq!(sections.get(name.as_str()).copied(), Some(1), "{name}: want exactly one section");
    }
    for name in sections.keys() {
        assert!(live.iter().any(|l| l == name), "section for {name} which tools/list does not serve");
    }
    assert_eq!(sections.len(), live.len());
}

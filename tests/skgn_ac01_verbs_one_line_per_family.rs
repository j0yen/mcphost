//! contract: reads registry handler::host_tool_descriptors
//! PRD-mcphost-plugin-skill-generated-from-registry
//! AC1 (P0) -- Given the registry, When `mcphost skill-gen --section verbs`
//! renders, Then every dotted family has exactly one line with its count and
//! verb names, the anonymous starter verbs are named, no per-verb
//! description appears, and the section is at most 60 lines.

use mcphost::handler::{host_tool_descriptors, starter_tool_names};
use mcphost::kinds::KindRegistry;
use mcphost::skill_gen;
use std::collections::BTreeMap;

#[test]
fn skgn_ac01_verbs_one_line_per_family() {
    let kinds = KindRegistry::with_builtin();
    let section = skill_gen::render_section("verbs", &kinds, None).expect("verbs renders");
    let lines: Vec<&str> = section.lines().collect();
    assert!(lines.len() <= 60, "verbs section is {} lines, limit is 60", lines.len());

    // Independent family count: canonical dotted names straight off the descriptors.
    let tools = host_tool_descriptors(&kinds);
    let mut families: BTreeMap<String, usize> = BTreeMap::new();
    for t in &tools {
        let name = t.name.as_ref();
        if !name.contains('.') || mcphost::tool_aliases::resolve(name).is_some() {
            continue;
        }
        let (family, _) = name.rsplit_once('.').unwrap();
        *families.entry(format!("{family}.*")).or_default() += 1;
    }
    assert!(families.len() > 20, "expected many families, found {}", families.len());
    for (family, count) in &families {
        let prefix = format!("- `{family}` ({count}): ");
        let hits = lines.iter().filter(|l| l.starts_with(&format!("- `{family}` "))).count();
        assert_eq!(hits, 1, "family {family} must have exactly one line");
        assert!(lines.iter().any(|l| l.starts_with(&prefix)), "family {family} line must carry count {count}");
    }
    assert_eq!(
        lines.iter().filter(|l| l.starts_with("- `")).count(),
        families.len(),
        "no line beyond one per family"
    );

    // Every verb leaf is named on its family's line.
    for t in &tools {
        let name = t.name.as_ref();
        if !name.contains('.') || mcphost::tool_aliases::resolve(name).is_some() {
            continue;
        }
        let (family, leaf) = name.rsplit_once('.').unwrap();
        let line = lines.iter().find(|l| l.starts_with(&format!("- `{family}.*` "))).unwrap();
        assert!(line.contains(leaf), "{name} missing from its family line: {line}");
    }

    // The anonymous starter verbs are named individually.
    let starters = starter_tool_names(&kinds);
    assert!(starters.len() >= 11, "starter set shrank to {}", starters.len());
    for s in &starters {
        assert!(section.contains(&format!("`{s}`")), "starter verb {s} not named");
    }

    // No per-verb description text.
    for t in &tools {
        let desc: String = t.description.as_deref().unwrap_or("").chars().take(40).collect();
        if desc.len() >= 40 {
            assert!(!section.contains(&desc), "description of {} leaked into the section", t.name);
        }
    }
}

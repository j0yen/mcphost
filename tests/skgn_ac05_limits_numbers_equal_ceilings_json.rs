//! AC5 (PRD-mcphost-plugin-skill-generated-from-tools-list) -- Given the
//! `limits` section, When rendered, Then every key of `plan.ceilings_json()`
//! for `free` appears with its value and no number in the section is typed
//! outside the generator (test: the section's numbers equal the JSON's).

use std::collections::BTreeSet;

use mcphost::plans::PlanCatalog;
use mcphost::skill_gen::{self, Section};

/// Every run of ASCII digits in `text`, as integers.
fn numbers_in(text: &str) -> BTreeSet<i64> {
    let mut out = BTreeSet::new();
    let mut cur = String::new();
    for ch in text.chars().chain(std::iter::once(' ')) {
        if ch.is_ascii_digit() {
            cur.push(ch);
        } else if !cur.is_empty() {
            out.insert(cur.parse().expect("digits"));
            cur.clear();
        }
    }
    out
}

/// The leaf `(dotted key, value)` pairs of a `ceilings_json` object.
fn leaves(json: &serde_json::Value) -> Vec<(String, i64)> {
    let mut out = Vec::new();
    for (key, value) in json.as_object().expect("ceilings_json is an object") {
        match value.as_object() {
            Some(inner) => {
                for (k, v) in inner {
                    out.push((format!("{key}.{k}"), v.as_i64().expect("budget leaf is a number")));
                }
            }
            None => out.push((key.clone(), value.as_i64().expect("ceiling is a number"))),
        }
    }
    out
}

#[test]
fn skgn_ac05() {
    let catalog = PlanCatalog::default_catalog();
    let free = catalog.get("free").expect("free plan").ceilings_json();
    let pro = catalog.get("pro").expect("pro plan").ceilings_json();
    let section = skill_gen::render_limits(&catalog);

    assert!(section.starts_with(&Section::Limits.start_marker()));
    assert!(section.ends_with(&Section::Limits.end_marker()));

    // Every key of the free ceilings appears with its value, and pro's beside it.
    let free_leaves = leaves(&free);
    let pro_leaves: std::collections::BTreeMap<String, i64> = leaves(&pro).into_iter().collect();
    assert!(free_leaves.len() > 5, "free carries real ceilings");
    for (key, value) in &free_leaves {
        let pro_value = pro_leaves.get(key).map_or("-".to_string(), i64::to_string);
        let row = format!("| `{key}` | {value} | {pro_value} |");
        assert!(section.contains(&row), "limits section is missing `{row}`");
    }
    assert!(section.contains("| `budget.max_tool_latency_ms` | "), "the nested budget keys are dotted");

    // No number is typed outside the generator: every number in the section
    // (marker lines and prose included) is one of the JSON's values.
    let allowed: BTreeSet<i64> = free_leaves.iter().map(|(_, v)| *v).chain(pro_leaves.values().copied()).collect();
    let in_section = numbers_in(&section);
    let stray: Vec<&i64> = in_section.difference(&allowed).collect();
    assert!(stray.is_empty(), "numbers in the limits section that are not ceilings_json values: {stray:?}");
    assert_eq!(in_section, allowed, "the section's numbers equal the JSON's");

    // The checked-in files carry exactly this section.
    for path in [skill_gen::SKILL_PATH, skill_gen::WWW_SKILL_PATH] {
        let content = std::fs::read_to_string(path).expect("read skill");
        assert!(content.contains(&section), "{path} does not carry the rendered limits section");
    }
}

//! PRD-mcphost-plugin-skill-generated-from-registry
//! AC5 (P0) -- Given the `limits` section, When rendered, Then every key of
//! `plan.ceilings_json()` for `free` appears with its value and no number in
//! the section is typed outside the generator (test: the section's numbers
//! equal the JSON's).

use mcphost::plans::PlanCatalog;
use mcphost::skill_gen;
use serde_json::Value;
use std::collections::BTreeMap;

/// `ceilings_json` walked into dotted keys, independent of `ceilings_flat`.
fn dotted(prefix: &str, v: &Value, out: &mut BTreeMap<String, i64>) {
    match v {
        Value::Object(m) => {
            for (k, inner) in m {
                let key = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                dotted(&key, inner, out);
            }
        }
        Value::Number(n) => {
            out.insert(prefix.to_string(), n.as_i64().expect("integer ceiling"));
        }
        other => panic!("{prefix}: unexpected ceiling shape {other}"),
    }
}

#[test]
fn skgn_ac05_limits_numbers_equal_the_ceilings_json() {
    let catalog = PlanCatalog::default_catalog();
    let section = skill_gen::render_section("limits", &mcphost::kinds::KindRegistry::with_builtin(), None).unwrap();
    let mut want_free = BTreeMap::new();
    dotted("", &catalog.get("free").unwrap().ceilings_json(), &mut want_free);
    let mut want_pro = BTreeMap::new();
    dotted("", &catalog.get("pro").unwrap().ceilings_json(), &mut want_pro);
    assert!(want_free.len() > 10, "free ceilings found: {}", want_free.len());

    let mut numbers_in_section: Vec<String> = Vec::new();
    for key in want_free.keys() {
        let prefix = format!("- `{key}`: ");
        let line = section.lines().find(|l| l.starts_with(&prefix)).unwrap_or_else(|| panic!("no line for {key}"));
        assert!(line.contains(&format!("free {}", want_free[key])), "{key}: {line}");
        let pro = want_pro.get(key).map_or("-".to_string(), i64::to_string);
        assert!(line.ends_with(&format!("pro {pro}")), "{key}: {line}");
        numbers_in_section.extend(line[prefix.len()..].split(|c: char| !c.is_ascii_digit()).filter(|d| !d.is_empty()).map(String::from));
    }
    // No number anywhere else in the section (heading, prose, markers, key names).
    let outside: String = section
        .lines()
        .filter(|l| !l.starts_with("- `"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!outside.chars().any(|c| c.is_ascii_digit()), "a number is typed outside the ceiling lines: {outside}");

    let mut expected: Vec<String> = want_free.values().map(i64::to_string).collect();
    expected.extend(want_free.keys().map(|k| want_pro.get(k).map_or("-".to_string(), i64::to_string)).filter(|v| v != "-"));
    expected.sort();
    numbers_in_section.sort();
    assert_eq!(numbers_in_section, expected, "the section's numbers equal the JSON's");
}

//! PRD-mcphost-delegation-recipes-and-quickstart
//! AC4 (P0) — Given the registry gains a scope variant in a test build, When
//! `mcphost llms-txt` runs, Then the recipe section and the Scopes table both
//! change and no hand edit is needed (asserted by a golden diff).
//!
//! A new `Scope` variant is one more `ScopeRow` in `scopes::catalog()`; the
//! test feeds the generators the live catalog plus that row, using the same
//! `splice_recipe_sections` the `llms-txt` subcommand calls.

use mcphost::llms_txt::{
    RECIPE_END, RECIPE_START, SCOPES_END, SCOPES_START, render_recipe, render_scopes_section, splice_recipe_sections,
};
use mcphost::delegation_recipe::sub_agent_key_for;
use mcphost::scopes::{ScopeRow, catalog};

const LLMS_TXT: &str = include_str!("../www/llms.txt");

fn block<'a>(s: &'a str, start: &str, end: &str) -> &'a str {
    let a = s.find(start).expect("start marker");
    let b = s.find(end).expect("end marker") + end.len();
    &s[a..b]
}

fn without<'a>(s: &str, blocks: &[(&str, &str)]) -> String {
    let mut out = s.to_string();
    for (start, end) in blocks {
        let a = out.find(start).unwrap();
        let b = out.find(end).unwrap() + end.len();
        out.replace_range(a..b, "");
    }
    out
}

fn with_new_scope() -> Vec<ScopeRow> {
    let mut cat = catalog();
    let implied_by: Vec<String> = vec!["mcp".into(), "tools:admin".into()];
    cat.push(ScopeRow { name: "tools:admin".into(), implied_by });
    cat
}

#[test]
fn dlrec_ac04_unchanged_registry_is_a_noop() {
    let cat = catalog();
    let out = splice_recipe_sections(
        LLMS_TXT,
        &render_scopes_section(&cat),
        &render_recipe(&sub_agent_key_for(&cat)),
    );
    assert_eq!(out, LLMS_TXT);
}

#[test]
fn dlrec_ac04_new_scope_changes_the_table_and_the_recipe_and_nothing_else() {
    let cat = with_new_scope();
    let out = splice_recipe_sections(
        LLMS_TXT,
        &render_scopes_section(&cat),
        &render_recipe(&sub_agent_key_for(&cat)),
    );
    let blocks = [(SCOPES_START, SCOPES_END), (RECIPE_START, RECIPE_END)];

    // Golden diff, part 1: outside the two generated blocks, byte-identical.
    assert_eq!(without(&out, &blocks), without(LLMS_TXT, &blocks), "no hand edit, nothing else moves");

    // Part 2: the Scopes table gained exactly the new row.
    let (old_t, new_t) = (block(LLMS_TXT, SCOPES_START, SCOPES_END), block(&out, SCOPES_START, SCOPES_END));
    assert_ne!(old_t, new_t);
    let added: Vec<&str> = new_t.lines().filter(|l| !old_t.lines().any(|o| o == *l)).collect();
    assert_eq!(added, vec!["| `tools:admin` | `mcp`, `tools:admin` |"]);

    // Part 3: the recipe now names the new scope as one the helper cannot get.
    let (old_r, new_r) = (block(LLMS_TXT, RECIPE_START, RECIPE_END), block(&out, RECIPE_START, RECIPE_END));
    assert_ne!(old_r, new_r);
    assert!(new_r.contains("`tools:write`, `tools:admin`"), "{new_r}");
    assert!(!old_r.contains("tools:admin"));
}

//! PRD-mcphost-empty-call-refusal-stated-in-schema
//! contract: reads registry handler::host_tool_descriptors
//! AC5 (P0) -- Given the PR head, When `docs/tools.md`, `docs/openapi.json` and
//! the contract dump are regenerated, Then both constraints appear in the
//! generated docs, the committed files equal their regeneration, and the
//! contract diff reports the two `anyOf` additions as additive.

use mcphost::api_contract::{COMMITTED_CONTRACT, contract_version_string, diff, dump_contract};
use mcphost::kinds::KindRegistry;
use serde_json::{Value, json};
use std::path::Path;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const EITHER_OR: &[(&str, [&str; 2])] = &[("host.import", ["run_id", "url"]), ("host.key.delegate", ["scopes", "bind"])];

fn branches(alts: &[&str; 2]) -> Value {
    json!([{"required": [alts[0]]}, {"required": [alts[1]]}])
}

fn tool<'a>(doc: &'a Value, name: &str) -> &'a Value {
    doc["tools"].as_array().expect("tools").iter().find(|t| t["name"] == name).unwrap_or_else(|| panic!("{name} not in the document"))
}

#[test]
fn ecs_ac05_tools_doc_names_both_alternatives() {
    let kinds = KindRegistry::with_builtin();
    let committed = std::fs::read_to_string(Path::new(ROOT).join("docs/tools.md")).expect("docs/tools.md");
    assert_eq!(mcphost::gendocs::render_tools_markdown(&kinds), committed, "docs/tools.md is stale: run `mcphost gen-docs`");
    for (name, alts) in EITHER_OR {
        let start = committed.find(&format!("### `{name}`\n")).unwrap_or_else(|| panic!("no section for {name}"));
        let rest = &committed[start..];
        let section = &rest[..rest[4..].find("\n### ").map_or(rest.len(), |i| i + 4)];
        for alt in alts {
            assert!(section.contains(&format!("`{alt}`")), "{name}: docs/tools.md omits `{alt}`");
        }
        assert!(section.contains("one of"), "{name}: docs/tools.md omits the either-or marker");
    }
}

#[test]
fn ecs_ac05_contract_dump_and_openapi_carry_the_any_of() {
    let committed: Value = serde_json::from_str(COMMITTED_CONTRACT).expect("committed contract");
    let live = dump_contract(&KindRegistry::with_builtin());
    assert_eq!(committed, live, "contracts/host-tools.v1.json is stale: run `mcphost contract dump`");
    let openapi = std::fs::read_to_string(Path::new(ROOT).join("docs/openapi.json")).expect("docs/openapi.json");
    for (name, alts) in EITHER_OR {
        assert_eq!(tool(&committed, name)["input_schema"]["anyOf"], branches(alts), "{name}: contract anyOf");
    }
    // OpenAPI carries the contract by version stamp, so it moves with the
    // contract: a stale stamp means the `anyOf` additions were not regenerated.
    let doc: Value = serde_json::from_str(&openapi).expect("openapi json");
    assert_eq!(doc["info"]["x-contract-version"], json!(contract_version_string(&KindRegistry::with_builtin())), "docs/openapi.json is stale: run `mcphost gen openapi`");
}

#[test]
fn ecs_ac05_the_any_of_additions_are_additive_in_the_contract_diff() {
    let live = dump_contract(&KindRegistry::with_builtin());
    // The contract as it was before this change: no `anyOf` on the two tools.
    let mut before = live.clone();
    for t in before["tools"].as_array_mut().expect("tools") {
        if EITHER_OR.iter().any(|(n, _)| t["name"] == *n) {
            t["input_schema"].as_object_mut().expect("schema").remove("anyOf");
        }
    }
    assert_ne!(before, live, "the fixture must differ from the live contract");
    let violations = diff(&before, &live, &[], 0);
    assert!(violations.is_empty(), "the anyOf additions are breaking: {violations:?}");
}

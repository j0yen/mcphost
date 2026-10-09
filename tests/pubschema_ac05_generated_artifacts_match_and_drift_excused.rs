//! PRD-mcphost-publish-schema-from-registry
//! AC5 (P0) -- Given the PR, When `mcphost contract dump`, `gen-docs` and
//! `scripts/gen-llms-full.sh` are run, Then their outputs equal the
//! committed `contracts/host-tools.v1.json`, `docs/tools.md` and
//! `www/llms-full.txt`, and the contract-drift check passes with a
//! `contracts/deprecations.json` entry naming this PRD for the `kind` enum.

use mcphost::api_contract::{Violation, diff, dump_contract, dump_contract_bytes, load_deprecations};
use mcphost::kinds::KindRegistry;
use serde_json::Value;
use std::path::Path;
use std::process::Command;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn contract_dump_and_tools_doc_equal_the_committed_files() {
    // The same registry `mcphost contract dump` / `gen-docs` build.
    let kinds = KindRegistry::with_builtin();
    let committed = std::fs::read(Path::new(ROOT).join("contracts/host-tools.v1.json")).expect("contract");
    assert!(
        dump_contract_bytes(&kinds) == committed,
        "contracts/host-tools.v1.json is stale: run `mcphost contract dump`"
    );
    let docs = std::fs::read_to_string(Path::new(ROOT).join("docs/tools.md")).expect("tools doc");
    assert!(
        mcphost::gendocs::render_tools_markdown(&kinds) == docs,
        "docs/tools.md is stale: run `mcphost gen-docs`"
    );
}

#[test]
fn llms_full_equals_the_script_output() {
    let status = Command::new("bash")
        .arg("scripts/gen-llms-full.sh")
        .arg("--check")
        .current_dir(ROOT)
        .status()
        .expect("run gen-llms-full.sh --check");
    assert!(status.success(), "www/llms-full.txt is stale: run scripts/gen-llms-full.sh");
}

#[test]
fn the_kind_enum_narrowing_is_flagged_without_and_excused_with_the_deprecations_entry() {
    let live = dump_contract(&KindRegistry::with_builtin());
    // The previous contract: `kind` was a bare string (no enum) and the
    // schema carried no per-kind branches.
    let mut old = live.clone();
    for tool in old["tools"].as_array_mut().expect("tools") {
        if tool["name"] == "host.tool_publish" {
            let schema = tool["input_schema"].as_object_mut().expect("schema");
            schema.remove("allOf");
            schema["properties"]["kind"].as_object_mut().expect("kind").remove("enum");
        }
    }
    let now = mcphost::state::now_unix();

    let bare = diff(&old, &live, &[], now);
    assert!(
        bare.iter().any(|v| matches!(v, Violation::Narrowed { path, detail }
            if path == "host.tool_publish.kind" && detail.contains("enum added"))),
        "without an entry the new enum must be flagged: {bare:?}"
    );

    let deprecations = load_deprecations(&Path::new(ROOT).join("contracts/deprecations.json")).expect("load");
    let entry = deprecations
        .iter()
        .find(|d| d.path == "host.tool_publish.kind")
        .expect("deprecations.json has an entry for host.tool_publish.kind");
    assert!(entry.replacement.contains("PRD-mcphost-publish-schema-from-registry"), "{entry:?}");
    assert!(entry.lead_time_valid(), "{entry:?}");
    let excused = diff(&old, &live, &deprecations, now);
    assert!(excused.is_empty(), "the entry must excuse the narrowing: {excused:?}");

    // And the committed contract itself diffs clean against the live one.
    let committed: Value = serde_json::from_slice(
        &std::fs::read(Path::new(ROOT).join("contracts/host-tools.v1.json")).expect("contract"),
    )
    .expect("json");
    assert!(diff(&committed, &live, &deprecations, now).is_empty());
}

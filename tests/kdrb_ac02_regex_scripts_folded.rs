//! PRD-mcphost-kind-docs-from-registry-b AC2: the two regex doc-checkers are
//! gone from `scripts/`, and the sandbox-API block they policed is rendered
//! and checked by `mcphost gen-docs kinds` in every file that carries it.

use std::path::Path;

use mcphost::gendocs::{SANDBOX_API_TARGETS, render_sandbox_api_block, run_kinds};
use mcphost::kinds::production_registry;
use mcphost::plans::PlanCatalog;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn kdrb_ac02_scripts_are_absent() {
    let listed: Vec<String> = std::fs::read_dir(root().join("scripts"))
        .expect("list scripts/")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    for gone in ["sandbox-api-doc-check.sh", "spec-fields-doc-check.sh"] {
        assert!(!listed.iter().any(|f| f == gone), "scripts/{gone} must be deleted; scripts/ has {listed:?}");
    }
}

#[test]
fn kdrb_ac02_committed_sandbox_blocks_match_the_registry_render() {
    let stale = run_kinds(root(), &production_registry(), &PlanCatalog::default_catalog(), false).expect("run_kinds");
    assert!(stale.is_empty(), "stale blocks: {}", stale.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("; "));
    let rendered = render_sandbox_api_block();
    for rel in SANDBOX_API_TARGETS {
        let text = std::fs::read_to_string(root().join(rel)).expect("read target");
        assert!(text.contains(&rendered), "{rel} does not carry the rendered sandbox-API block");
    }
    // The skill file bridgedisc_ac01 reads carries one `mcphost.<module>` bullet per BRIDGE_MODULES entry.
    let skill = std::fs::read_to_string(root().join("plugin/skills/mcphost/SKILL.md")).expect("read SKILL.md");
    for m in mcphost::kinds::python::BRIDGE_MODULES {
        assert!(skill.contains(&format!("`mcphost.{}`", m.name)), "SKILL.md block is missing mcphost.{}", m.name);
    }
}

#[test]
fn kdrb_ac02_a_block_missing_a_module_is_named_stale() {
    let scratch = std::env::temp_dir().join(format!("kdrb-ac02-{}-{}", std::process::id(), mcphost::state::now_unix_ms()));
    for rel in SANDBOX_API_TARGETS.iter().copied().chain(["docs/kinds/chain.md", "docs/kinds/echo.md", "docs/kinds/http.md", "docs/kinds/wasm.md"]) {
        let dest = scratch.join(rel);
        std::fs::create_dir_all(dest.parent().expect("parent")).expect("mkdir");
        std::fs::copy(root().join(rel), &dest).expect("copy");
    }
    let skill = scratch.join("plugin/skills/mcphost/SKILL.md");
    let first = mcphost::kinds::python::BRIDGE_MODULES[0].name;
    let text = std::fs::read_to_string(&skill).expect("read");
    let cut: Vec<&str> = text.lines().filter(|l| !l.starts_with(&format!("- `mcphost.{first}`"))).collect();
    std::fs::write(&skill, cut.join("\n") + "\n").expect("write");
    let stale = run_kinds(&scratch, &production_registry(), &PlanCatalog::default_catalog(), false).expect("run_kinds");
    let _ = std::fs::remove_dir_all(&scratch);
    assert_eq!(stale.len(), 1, "{stale:?}");
    assert_eq!(stale[0].page, "plugin/skills/mcphost/SKILL.md");
    assert_eq!(stale[0].block, "mcphost-sandbox-api");
}

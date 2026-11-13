//! contract: reads registry kinds::python::BRIDGE_MODULES
//! PRD-mcphost-kind-docs-from-registry-b-v2 AC1: `gen-docs kinds` renders the
//! `mcphost-sandbox-api` block of the four files that carry it from the
//! python kind's `KindDoc.bridge_modules`; a fixture module lands in all four,
//! and for the real module set the committed blocks are what it renders.

use std::path::{Path, PathBuf};
use std::process::Command;

use mcphost::gendocs::{
    SANDBOX_API_END, SANDBOX_API_START, SANDBOX_API_TARGETS, render_sandbox_api_block, run_kind_docs, run_kinds,
};
use mcphost::kinds::kinddoc::{BridgeDoc, KindDoc};
use mcphost::plans::PlanCatalog;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// A scratch tree holding every page and file `gen-docs kinds` reads.
fn scratch(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kdrb_ac01_{label}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("docs/kinds")).unwrap();
    for entry in std::fs::read_dir(root().join("docs/kinds")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dir.join("docs/kinds").join(entry.file_name())).unwrap();
    }
    for rel in SANDBOX_API_TARGETS {
        let dest = dir.join(rel);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::copy(root().join(rel), dest).unwrap();
    }
    dir
}

fn marked(text: &str) -> &str {
    let s = text.find(SANDBOX_API_START).expect("start marker");
    let e = text.find(SANDBOX_API_END).expect("end marker") + SANDBOX_API_END.len();
    &text[s..e]
}

fn docs() -> Vec<KindDoc> {
    let registry = mcphost::kinds::production_registry();
    let plans = PlanCatalog::default_catalog();
    registry.names().into_iter().filter_map(|n| registry.get(n)).map(|k| k.doc(&plans)).collect()
}

#[test]
fn kdrb_ac01_sandbox_api_block_from_registry() {
    // A fixture module on top of BRIDGE_MODULES reaches all four files.
    let tmp = scratch("fixture");
    let mut docs = docs();
    let python = docs.iter_mut().find(|d| d.name == "python").expect("python kind");
    let modules = python.bridge_modules.as_mut().expect("python carries bridge modules");
    assert_eq!(modules.len(), mcphost::kinds::python::BRIDGE_MODULES.len());
    modules.push(BridgeDoc { name: "fixture_mod".into(), purpose: "a fixture module for AC1".into() });
    let stale = run_kind_docs(&tmp, &docs, true).expect("render the scratch tree");
    assert_eq!(stale.len(), SANDBOX_API_TARGETS.len(), "one stale sandbox-api block per file: {stale:?}");
    for rel in SANDBOX_API_TARGETS {
        let text = std::fs::read_to_string(tmp.join(rel)).unwrap();
        assert!(
            marked(&text).contains("- `mcphost.fixture_mod` -- a fixture module for AC1\n"),
            "{rel} lacks the fixture bullet inside the markers"
        );
        assert_eq!(text.matches(SANDBOX_API_START).count(), 1, "{rel}: markers stay single");
        let before = std::fs::read_to_string(root().join(rel)).unwrap();
        assert_eq!(
            text.replace(marked(&text), ""),
            before.replace(marked(&before), ""),
            "{rel}: bytes outside the markers are untouched"
        );
    }
    assert!(run_kind_docs(&tmp, &docs, false).unwrap().is_empty(), "a second render is a no-op");
    let _ = std::fs::remove_dir_all(&tmp);

    // The real set: the committed blocks are what the registry renders, and
    // `gen-docs kinds --check` passes on a copy of the tree.
    let registry = mcphost::kinds::production_registry();
    let plans = PlanCatalog::default_catalog();
    let stale = run_kinds(root(), &registry, &plans, false).expect("check the real tree");
    assert!(stale.is_empty(), "committed blocks differ from the registry's render: {stale:?}");
    let python = registry.get("python").unwrap().doc(&plans);
    let want = render_sandbox_api_block(python.bridge_modules.as_deref().unwrap());
    for rel in SANDBOX_API_TARGETS {
        let text = std::fs::read_to_string(root().join(rel)).unwrap();
        assert_eq!(marked(&text), want, "{rel}: committed block is byte-identical to the render");
    }
    let tmp = scratch("real");
    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["gen-docs", "kinds", "--check"])
        .current_dir(&tmp)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let _ = std::fs::remove_dir_all(&tmp);
}

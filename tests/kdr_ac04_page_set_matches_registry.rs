//! PRD-mcphost-kind-docs-from-registry AC4: every registered kind has a
//! `docs/kinds/<kind>.md` page and every page has a registered kind.

use async_trait::async_trait;
use mcphost::gendocs::check_kind_page_set;
use mcphost::kinds::{CallCtx, Kind, KindError, KindRegistry, ToolDescriptor, production_registry};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;

struct FakeKind;

#[async_trait]
impl Kind for FakeKind {
    fn contract(&self) -> &'static mcphost::kinds::contract::KindContract {
        &mcphost::kinds::contract::EMPTY_CONTRACT
    }
    fn name(&self) -> &'static str {
        "fake"
    }
    fn validate(&self, _spec: &Value) -> Result<(), KindError> {
        Ok(())
    }
    fn describe(&self, _spec: &Value) -> ToolDescriptor {
        ToolDescriptor { name: "fake".into(), description: "fixture".into(), input_schema: json!({"type": "object"}) }
    }
    async fn call(&self, _spec: &Value, args: Value, _ctx: &CallCtx) -> Result<Value, KindError> {
        Ok(args)
    }
}

fn repo() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn kdr_ac04_page_set_matches_registry() {
    // The real tree: five kinds, five pages.
    check_kind_page_set(repo(), &production_registry()).expect("the repo's pages match its kinds");

    // A sixth kind with no page fails naming it.
    let mut with_fake: KindRegistry = production_registry();
    with_fake.register(Arc::new(FakeKind));
    let errs = check_kind_page_set(repo(), &with_fake).expect_err("fake has no page");
    assert!(errs.iter().any(|e| e.contains("fake")), "{errs:?}");

    // A stray page with no kind fails naming it.
    let tmp = std::env::temp_dir().join(format!("kdr_ac04_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let pages = tmp.join("docs/kinds");
    std::fs::create_dir_all(&pages).unwrap();
    for entry in std::fs::read_dir(repo().join("docs/kinds")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), pages.join(entry.file_name())).unwrap();
    }
    std::fs::write(pages.join("old.md"), "stray\n").unwrap();
    let errs = check_kind_page_set(&tmp, &production_registry()).expect_err("old has no kind");
    assert!(errs.iter().any(|e| e.contains("old")), "{errs:?}");
    let _ = std::fs::remove_dir_all(&tmp);
}

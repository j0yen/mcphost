//! PRD-mcphost-tool-directory-format
//! AC5 (P0) — Given `examples/tools/hello-py/`, When `pack` runs, Then the
//! payload's python source equals the bytes of `tool.py` and `spec` has no
//! `tests` key (tests are not part of the publish payload).
//!
//! The python kind's code field is `source` (`known_spec_fields("python")`),
//! not `code`: a `code` key would be refused `unknown_spec_field`, so the
//! payload carries the file under `spec.source`. Also covers R5: both
//! checked-in examples pass `lint`.

use mcphost::kinds::production_registry;
use mcphost::tooldir::{lint, pack};
use serde_json::json;
use std::path::Path;

fn example(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/tools").join(name)
}

#[test]
fn tdf_ac05_hello_py_source_is_tool_py_and_tests_are_not_published() {
    let dir = example("hello-py");
    assert!(dir.join("tests").is_dir(), "hello-py carries a declared test");
    let payload = pack(&dir).expect("hello-py packs");
    let on_disk = std::fs::read(dir.join("tool.py")).unwrap();
    assert_eq!(payload["spec"]["source"].as_str().unwrap().as_bytes(), on_disk.as_slice());
    assert!(payload["spec"].get("tests").is_none(), "tests are not part of the publish payload");
    assert!(payload.get("tests").is_none());
    assert_eq!(payload["name"], json!("hello_py"));
    assert_eq!(payload["kind"], json!("python"));
}

#[test]
fn tdf_ac05_both_examples_pass_lint() {
    let registry = production_registry();
    for name in ["hello", "hello-py"] {
        if let Err(e) = lint(&example(name), &registry) {
            panic!("{name}: {}", e.message);
        }
    }
}

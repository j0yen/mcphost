//! PRD-mcphost-tool-infer AC17 (P0) — Given publish-time inference on any
//! source, When it runs, Then no network request is made and no tenant
//! code is executed.
//!
//! "No network request" holds structurally: `mcphost::kinds::infer` never
//! imports `reqwest`/`tokio::net`/`std::net` and its functions take only a
//! `&str`/`&[(String, String)]` and return a `Result<Value, KindError>` --
//! there is nothing in that signature able to reach the network (verified
//! here by grepping the module's own source, not just asserted in prose).
//! "No tenant code executed" is the testable half: a source whose
//! top-level statement has an observable side effect if actually run
//! (writing a marker file) must NOT have that side effect after inference
//! runs over it -- proving the "static analysis" in requirement 13 really
//! is static.

use mcphost::kinds::infer::{infer_python_args_schema, infer_python_requirements};
use std::path::PathBuf;

#[test]
fn inference_module_source_never_references_network_types() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/kinds/infer.rs"))
        .expect("read infer.rs");
    for needle in ["reqwest", "tokio::net", "std::net", "TcpStream", "UdpSocket"] {
        assert!(
            !src.contains(needle),
            "kinds::infer must never reference {needle} -- it would reopen a network path in a synchronous, publish-time-gating function"
        );
    }
}

#[test]
fn a_source_with_a_side_effecting_top_level_statement_is_never_executed() {
    let marker: PathBuf = std::env::temp_dir().join(format!(
        "mcphost-infer-ac17-marker-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&marker);

    let source = format!(
        "open({:?}, \"w\").close()\n\ndef main(args):\n    return args[\"city\"]\n",
        marker.to_string_lossy()
    );

    // Both inference entry points run over the source; neither must ever
    // execute it.
    let schema = infer_python_args_schema(&source).expect("schema inference ok");
    let requirements = infer_python_requirements(&source).expect("requirements inference ok");

    assert!(
        !marker.exists(),
        "inference must be static analysis only -- it must never execute the tenant's source"
    );
    assert_eq!(schema["required"], serde_json::json!(["city"]));
    assert!(requirements.is_empty());

    let _ = std::fs::remove_file(&marker);
}

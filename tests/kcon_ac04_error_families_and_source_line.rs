//! AC4 — Given a python tool that raises on line 3, When called, Then the
//! error carries `data.source_line` and `data.families: ["tool_code"]`;
//! Given a wasm component that traps, Then `tool_trapped` carries
//! `data.families: ["tool_code"]` and no `source_line` key.

use std::time::{Duration, Instant};

use base64::Engine as _;
use mcphost::kinds::python::PythonKind;
use mcphost::kinds::wasm::WasmKind;
use mcphost::kinds::{CallCtx, Kind, KindError};
use mcphost::sandbox;
use serde_json::{Value, json};

use crate::common;

async fn call_until_built(kind: &dyn Kind, spec: &Value) -> KindError {
    let ctx = CallCtx::for_test(1, "t_kcon_ac04");
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match kind.call(spec, json!({}), &ctx).await {
            Err(KindError::Structured { code: "tool_building", .. }) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(e) => return e,
            Ok(v) => panic!("the call must fail, got {v}"),
        }
    }
}

#[tokio::test]
async fn kcon_ac04_python_error_carries_source_line_and_tool_code_family() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let uv_present = std::process::Command::new("uv")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false);
    if !uv_present {
        println!("skipped: no uv");
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let kind = PythonKind::new(&envs_dir.0);
    // The raise is on line 3 of the tool source.
    let spec = json!({"source": "def main(args):\n    x = 1\n    raise ValueError('boom')\n"});

    let KindError::Structured { code, data, .. } = call_until_built(&kind, &spec).await else {
        panic!("python error must be structured");
    };
    assert_eq!(code, "tool_exception");
    assert_eq!(data["families"], json!(["tool_code"]));
    assert_eq!(data["line"], json!(3));
    assert_eq!(
        data["source_line"].as_str(),
        Some("raise ValueError('boom')"),
        "data.source_line is the offending source line: {data}"
    );
}

#[tokio::test]
async fn kcon_ac04_wasm_trap_carries_tool_code_family_and_no_source_line() {
    let bytes = include_bytes!("fixtures/wasm/trap.wasm");
    let spec = json!({"component": base64::engine::general_purpose::STANDARD.encode(bytes)});

    let KindError::Structured { code, data, .. } = call_until_built(&WasmKind::new(), &spec).await else {
        panic!("wasm trap must be structured");
    };
    assert_eq!(code, "tool_trapped");
    assert_eq!(data["families"], json!(["tool_code"]));
    assert!(
        data.as_object().is_some_and(|o| !o.contains_key("source_line")),
        "a wasm trap has no source_line key: {data}"
    );
}

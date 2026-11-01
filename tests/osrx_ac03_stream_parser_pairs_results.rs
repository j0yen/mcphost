//! contract: reads scripts/live/osreach_import_reach.py
//! PRD-mcphost-import-dry-run-live-real-export
//! AC3 (P0) -- Given a stream-json transcript with a paired `host_import` result
//! marked `is_error`, a paired `host_doctor` result naming `unreached` and
//! `unreached_cap`, and one unpaired `tool_result`, When the driver's parser
//! reads it, Then it counts `tool_results=2`, `is_error=1`, reads `unreached`
//! and `cap` from the doctor text, ignores the unpaired block, and returns
//! result `fail`.

use serde_json::{Value, json};
use std::process::Command;

fn assistant_use(id: &str, name: &str) -> Value {
    json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": id, "name": name, "input": {}}]}})
}

fn user_result(id: &str, text: &str, is_error: bool) -> Value {
    json!({"type": "user", "message": {"content": [
        {"type": "tool_result", "tool_use_id": id, "content": [{"type": "text", "text": text}], "is_error": is_error}
    ]}})
}

#[test]
fn osrx_ac03_parser_counts_paired_results_and_ignores_the_unpaired_block() {
    let lines = [
        json!({"type": "system", "subtype": "init"}),
        assistant_use("tu_1", "mcp__mcphost__host_import"),
        user_result("tu_1", "no completed host.export matches that run_id / url", true),
        assistant_use("tu_2", "mcp__mcphost__host_doctor"),
        user_result("tu_2", "{\"output_reach\": {\"unreached\": 654, \"unreached_cap\": 700}}", false),
        // A result whose tool_use_id pairs with no judged tool_use.
        user_result("tu_orphan", "Structured content does not match the tool's output schema", true),
    ];
    let dir = std::env::temp_dir().join(format!("osrx_ac03_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let stream = dir.join("stream.jsonl");
    let body: Vec<String> = lines.iter().map(Value::to_string).collect();
    std::fs::write(&stream, body.join("\n")).expect("write stream");

    let out = Command::new("python3")
        .arg(super::osrx_ac02_plan_mode_offline::SCRIPT)
        .arg("--parse")
        .arg(&stream)
        .output()
        .expect("run parser");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let parsed: Value = serde_json::from_slice(&out.stdout).expect("parser prints one JSON object");

    assert_eq!(parsed["tool_results"], json!(2), "{parsed}");
    assert_eq!(parsed["is_error"], json!(1), "{parsed}");
    assert_eq!(parsed["mismatch"], json!(0), "the unpaired mismatch text is ignored: {parsed}");
    assert_eq!(parsed["unreached"], json!(654), "{parsed}");
    assert_eq!(parsed["cap"], json!(700), "{parsed}");
    assert_eq!(parsed["result"], json!("fail"), "{parsed}");
}

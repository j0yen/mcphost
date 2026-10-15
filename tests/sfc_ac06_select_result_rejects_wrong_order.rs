//! PRD-mcphost-sse-frame-contract-generated AC6 (P0) -- Given a captured body
//! `[result, notifications/tools/list_changed]` (wrong order), When
//! `frames::select_result` is applied with `tools/call:keyless_signup`, Then
//! it returns `frame_contract_violation` listing both methods; Given the
//! correct order, Then it returns the result frame.

use mcphost::protocol::frames::{FrameContractViolation, REQ_KEYLESS_SIGNUP, select_result, split_sse_frames};
use serde_json::json;

fn body(frames: &[serde_json::Value]) -> String {
    frames.iter().map(|f| format!("data: {f}\n\n")).collect()
}

#[test]
fn wrong_order_is_a_violation_naming_both_methods_and_right_order_returns_the_result() {
    let notification = json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"});
    let result = json!({"jsonrpc": "2.0", "id": 1, "result": {"ok": true}});

    let wrong = split_sse_frames(&body(&[result.clone(), notification.clone()]));
    let violation = select_result(&wrong, REQ_KEYLESS_SIGNUP).expect_err("wrong order must be rejected");
    let text = violation.to_string();
    assert!(text.contains(FrameContractViolation::CODE), "{text}");
    assert!(text.contains("frame_contract_violation"), "{text}");
    assert!(text.contains("notifications/tools/list_changed") && text.contains("result"), "must list both methods: {text}");
    assert_eq!(violation.observed, ["result", "notifications/tools/list_changed"]);

    let right = split_sse_frames(&body(&[notification, result.clone()]));
    assert_eq!(select_result(&right, REQ_KEYLESS_SIGNUP).expect("correct order"), &result);
}

//! PRD-mcphost-sandbox-pool-self-restart-v2 AC7: a killed wedged child that
//! wrote to stderr leaves `after.stderr_tails[0]` of at most 2048 bytes in the
//! row `host.incidents.list` returns, and `/incidents.json` omits the field.

use crate::common::EnvGuard;
use crate::spsr;

use serde_json::{Value, json};

#[tokio::test]
async fn spsr_ac07() {
    if mcphost::sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", mcphost::sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let _dry = EnvGuard::clear(mcphost::heal::DRY_RUN_ENV).await;
    let (state, python, _dir) = spsr::app(5).await;
    let noisy = json!({
        "source": "import os\ndef main(args):\n    os.write(2, b'E' * 6000 + b'LAST-WORDS')\n    return {'n': 1}\n",
        "args_schema": {"type": "object"},
        "requirements": [],
        "timeout_s": 1,
    });
    let wedge = spsr::wedge_with(&python, "noisy_tool", noisy).await;

    let _subscriber = mcphost::pool_restart::spawn_subscriber(state.clone());
    spsr::breach(&state);
    let row = spsr::wait_row(&state).await;
    assert_eq!(row.outcome.as_str(), "ok", "row: {row:?}");
    let _ = wedge.call.await;

    let listed = mcphost::heal::incidents_list(&state, &json!({"action": "sandbox_pool_restart"})).await.unwrap();
    let after: Value = serde_json::from_str(listed["incidents"][0]["after_json"].as_str().unwrap()).unwrap();
    let tail = after["stderr_tails"][0].as_str().unwrap_or_else(|| panic!("after: {after}"));
    assert!(!tail.is_empty() && tail.len() <= 2048, "tail is {} bytes", tail.len());
    assert!(tail.ends_with("LAST-WORDS"), "tail: {tail}");

    let public = mcphost::heal::incidents_json(&state).await.unwrap();
    assert!(public[0]["after"].get("stderr_tails").is_none(), "public row: {}", public[0]);
    assert!(!public.to_string().contains("LAST-WORDS"), "public: {public}");
}

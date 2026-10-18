//! PRD-mcphost-sampling-from-tools
//! AC5 (P0) — Given a free tenant at 19 990 sampled tokens today, When an ask
//! estimated at 20 tokens arrives, Then `sampling_quota_exceeded {limit:
//! 20000, reset_unix}` is returned, the fixture client's counter is
//! unchanged, and `host.quickstart.limits.plan.sampling_tokens_per_day` reads
//! 20000.
//!
//! The estimate is `max_tokens + len(prompt)/4`: `max_tokens=16` and a
//! 16-byte prompt make 20. The tenant's counter is seeded through the same
//! `Db::sampling_record` the bridge writes with.

use crate::common;
use crate::elicit_session;
use crate::sampling_session;
use common::{McpClient, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use sampling_session::{Answer, SamplingSession};
use serde_json::{Value, json};
use std::time::Duration;

const TOOL: &str = "import mcphost\ndef main(args):\n    if args.get(\"warm\"):\n        return {\"warm\": True}\n    try:\n        r = mcphost.sampling.ask(args[\"prompt\"], max_tokens=args[\"max_tokens\"])\n        return {\"text\": r[\"text\"], \"tokens\": r[\"tokens\"]}\n    except mcphost.sampling.SamplingError as e:\n        return {\"code\": e.code, \"limit\": e.data.get(\"limit\"), \"reset_unix\": e.data.get(\"reset_unix\")}\n";

async fn call_ready(client: &SamplingSession, name: &str, args: Value, answer: &Answer) -> Value {
    for _ in 0..200 {
        let resp = client.call(name, args.clone(), answer).await;
        let building = resp["result"].get("structuredContent").is_some_and(|s| s["status"] == json!("building"));
        if !building {
            return resp;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("tool never became ready");
}

#[tokio::test]
async fn an_ask_past_the_daily_ceiling_is_refused_and_the_client_is_never_asked() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs.0)).await;
    let (ns, key) = signup(&server.base_url, "Msmp AC5").await;
    let admin = McpClient::with_bearer(&server.base_url, &key);
    admin
        .tools_call("host.tool_publish", json!({"name": "triage", "kind": "python", "spec": {"source": TOOL, "timeout_s": 40}}))
        .await
        .expect("publish");
    let tenant = server.state.db.find_tenant_by_namespace(ns.clone()).await.expect("db").expect("tenant");
    assert_eq!(tenant.plan, "free");
    let day_start = mcphost::state::utc_midnight_unix(mcphost::state::now_unix());
    server.state.db.sampling_record(tenant.id, day_start, 19_990).await.expect("seed");

    let base = elicit_session::serve_session_mode(server.state.clone()).await;
    let client = SamplingSession::open(&base, &key, true).await;
    let answer = Answer::text("yes").with_usage(0, 1);
    let name = format!("{ns}.triage");
    call_ready(&client, &name, json!({"warm": true}), &answer).await;

    // 16 + 16/4 = 20 estimated tokens past 19 990: refused, the client untouched.
    let args = json!({"prompt": "0123456789abcdef", "max_tokens": 16});
    let out = extract_structured(&client.call(&name, args, &answer).await["result"]);
    assert_eq!(out["code"], json!("sampling_quota_exceeded"), "{out}");
    assert_eq!(out["limit"], json!(20_000), "{out}");
    assert_eq!(out["reset_unix"], json!(day_start + 86_400), "{out}");
    assert_eq!(client.received.lock().unwrap().len(), 0, "the client must not be asked");
    assert_eq!(client.answered.load(std::sync::atomic::Ordering::SeqCst), 0, "the client's counter is unchanged");

    // 4 + 1 = 5 estimated tokens fits under the ceiling; the client's own count (1) is what is recorded.
    let args = json!({"prompt": "abcd", "max_tokens": 4});
    let out = extract_structured(&client.call(&name, args, &answer).await["result"]);
    assert_eq!(out["text"], json!("yes"), "{out}");
    let usage = extract_structured(&admin.tools_call("host.usage", json!({})).await.expect("usage"));
    assert_eq!(usage["sampling_tokens_today"], json!(19_991), "{usage}");

    let quickstart = extract_structured(&admin.tools_call("host.quickstart", json!({})).await.expect("quickstart"));
    assert_eq!(quickstart["limits"]["plan"]["sampling_tokens_per_day"], json!(20_000), "{quickstart}");
}

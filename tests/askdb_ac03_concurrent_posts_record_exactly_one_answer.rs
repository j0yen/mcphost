//! PRD-mcphost-ask-user-decision-b
//! AC3 -- Given two concurrent POSTs of different choices to one ask, When
//! both complete, Then exactly one is recorded, the other is 409, and
//! `host.ask.get` shows one `answered` event.

use crate::common;

use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn racing_posts_record_one_answer_and_the_loser_is_409() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "askdb ac03 tenant").await;
    let agent = McpClient::with_bearer(&server.base_url, &key);

    let created = extract_structured(
        &agent
            .tools_call("host.ask.create", json!({"question": "Ship it?", "options": ["yes", "no"]}))
            .await
            .expect("ask.create"),
    );
    let ask_id = created["ask_id"].as_str().expect("ask_id").to_string();
    let advertised = created["url"].as_str().expect("url");
    let url = format!("{}/a/{}", server.base_url, advertised.split_once("/a/").expect("/a/ url").1);

    let http = reqwest::Client::new();
    let (a, b) = tokio::join!(
        http.post(&url).form(&[("choice", "yes")]).send(),
        http.post(&url).form(&[("choice", "no")]).send(),
    );
    let (a, b) = (a.expect("post yes"), b.expect("post no"));
    let mut statuses = [a.status().as_u16(), b.status().as_u16()];
    statuses.sort_unstable();
    assert_eq!(statuses, [200, 409], "exactly one POST is recorded, the other is 409");
    let winner = if a.status() == 200 { "yes" } else { "no" };

    let got = extract_structured(&agent.tools_call("host.ask.get", json!({"ask_id": ask_id})).await.expect("ask.get"));
    assert_eq!(got["status"], json!("answered"), "{got}");
    assert_eq!(got["choice"], json!(winner), "{got}");
    let events = got["events"].as_array().expect("events");
    let answered: Vec<_> = events.iter().filter(|e| e["kind"] == json!("answered")).collect();
    assert_eq!(answered.len(), 1, "exactly one answered event: {got}");
    assert_eq!(answered[0]["choice"], json!(winner), "{got}");
    assert_eq!(events.first().map(|e| e["kind"].clone()), Some(json!("created")), "{got}");
    assert_eq!(events.len(), 2, "created + answered, nothing else: {got}");

    let listed = extract_structured(&agent.tools_call("host.ask.list", json!({"status": "answered"})).await.expect("ask.list"));
    let asks = listed["asks"].as_array().expect("asks");
    assert_eq!(asks.len(), 1, "{listed}");
    assert_eq!(asks[0]["ask_id"], json!(ask_id), "{listed}");
    assert_eq!(asks[0]["options"], json!(["yes", "no"]), "{listed}");
    for key in ["question", "status", "choice", "created_unix", "answered_unix", "expires_unix", "waited_total_ms"] {
        assert!(asks[0].get(key).is_some(), "audit row must carry {key}: {listed}");
    }
}

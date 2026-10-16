//! PRD-mcphost-openapi-for-rest-facade
//! AC4 (P0) -- Given `POST /x/{token}/{tool}` responses for `200`, `202`,
//! `400`, `404`, `429`, `503` and `500` produced by the suite, When each is
//! validated against the documented response schema, Then all pass and the
//! error envelope schema requires exactly `code` and `message`.
//!
//! Every response here is a real one from a real server; the schema each is
//! checked against is read back out of `GET /openapi.json` (operation
//! `POST /x/{token}/{tool}`, `components.schemas`), not from the Rust structs,
//! so the document a client generator sees is what is proven.

use crate::common;
use async_trait::async_trait;
use common::{McpClient, extract_structured, signup};
use mcphost::kinds::{CallCtx, Kind, KindError, KindRegistry, ToolDescriptor};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

/// `mode` in the call args picks the outcome: `ok` returns at once, `slow`
/// outlasts the (shrunk) sync deadline, `fail` ends the run in error.
struct ProbeKind;

#[async_trait]
impl Kind for ProbeKind {
    fn contract(&self) -> &'static mcphost::kinds::contract::KindContract {
        &mcphost::kinds::contract::EMPTY_CONTRACT
    }

    fn name(&self) -> &'static str {
        "oapi_probe"
    }

    fn validate(&self, _spec: &Value) -> Result<(), KindError> {
        Ok(())
    }

    fn describe(&self, _spec: &Value) -> ToolDescriptor {
        ToolDescriptor {
            name: "oapi_probe".to_string(),
            description: "returns, sleeps or fails on `mode`".to_string(),
            input_schema: json!({"type": "object"}),
        }
    }

    async fn call(&self, _spec: &Value, args: Value, _ctx: &CallCtx) -> Result<Value, KindError> {
        match args["mode"].as_str() {
            Some("slow") => {
                tokio::time::sleep(Duration::from_secs(3)).await;
                Ok(json!({"slept": true}))
            }
            Some("fail") => Err(KindError::Exec("probe failure".to_string())),
            _ => Ok(json!({"echo": args})),
        }
    }
}

/// `schema_ref` resolved against the document: a root schema that carries the
/// document's `components` so `#/components/schemas/..` pointers resolve.
fn documented_validator(doc: &Value, status: u16) -> Option<jsonschema::Validator> {
    let op = &doc["paths"]["/x/{token}/{tool}"]["post"]["responses"][status.to_string()];
    assert!(op.is_object(), "status {status} is not documented for POST /x/{{token}}/{{tool}}");
    let schema = &op["content"]["application/json"]["schema"];
    if schema.is_null() {
        return None;
    }
    let mut root = schema.clone();
    root["components"] = doc["components"].clone();
    Some(jsonschema::validator_for(&root).expect("documented schema compiles"))
}

fn assert_documented(doc: &Value, status: u16, body: &Value) {
    let validator = documented_validator(doc, status).unwrap_or_else(|| panic!("status {status} has no documented JSON schema"));
    let errors: Vec<String> = validator.iter_errors(body).map(|e| e.to_string()).collect();
    assert!(errors.is_empty(), "{status} body {body} violates its documented schema: {errors:?}");
}

#[tokio::test]
async fn oapi_ac04() {
    let (mut state, _data_dir) = common::bare_app_state().await;
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(ProbeKind));
    state.kinds = kinds;
    // Below the probe's 3 s sleep, above a fast run's 200 ms poll tick.
    state.public_url_sync_deadline = Duration::from_millis(1500);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let base_url = format!("http://{}", listener.local_addr().expect("addr"));
    state.public_url = base_url.clone();
    let state = Arc::new(state);
    mcphost::runs::spawn_executor((*state).clone());
    let serve_state = state.clone();
    tokio::spawn(async move {
        let _ = mcphost::http::serve_on_listener(listener, serve_state).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let doc: Value = reqwest::get(format!("{base_url}/openapi.json"))
        .await
        .expect("GET /openapi.json")
        .json()
        .await
        .expect("openapi json");

    // The documented status set is exactly the seven the PRD names (+ none else).
    let documented: std::collections::BTreeSet<String> = doc["paths"]["/x/{token}/{tool}"]["post"]["responses"]
        .as_object()
        .expect("responses")
        .keys()
        .cloned()
        .collect();
    let want: std::collections::BTreeSet<String> = ["200", "202", "400", "404", "429", "500", "503"].map(String::from).into();
    assert_eq!(documented, want);

    // The error envelope requires exactly `code` and `message`.
    let mut required: Vec<&str> = doc["components"]["schemas"]["Error"]["required"]
        .as_array()
        .expect("Error.required")
        .iter()
        .map(|v| v.as_str().expect("string"))
        .collect();
    required.sort_unstable();
    assert_eq!(required, ["code", "message"]);
    let envelope = documented_validator(&doc, 400).expect("400 has a schema");
    assert!(!envelope.is_valid(&json!({"ok": false, "error": {"code": "x"}})), "message is required");
    assert!(!envelope.is_valid(&json!({"ok": false, "error": {"message": "x"}})), "code is required");

    let (tenant_ns, key) = signup(&base_url, "OAPI AC4 Tenant").await;
    let client = McpClient::with_bearer(&base_url, &key);
    client
        .tools_call("host.tool_publish", json!({"name": "probe", "kind": "oapi_probe", "spec": {}}))
        .await
        .expect("publish");
    let shared = extract_structured(
        &client
            .tools_call("host.tool_share", json!({"name": "probe", "visibility": "url"}))
            .await
            .expect("tool_share"),
    );
    let url = shared["url"].as_str().expect("url").to_string();
    let http = reqwest::Client::new();

    // 200
    let resp = http.post(&url).body(r#"{"mode":"ok"}"#).send().await.expect("POST 200");
    assert_eq!(resp.status(), 200);
    assert_documented(&doc, 200, &resp.json().await.expect("200 json"));

    // 202, and the poll URL it names answers 200 with the same shape once done.
    let resp = http.post(&url).body(r#"{"mode":"slow"}"#).send().await.expect("POST 202");
    assert_eq!(resp.status(), 202);
    let accepted: Value = resp.json().await.expect("202 json");
    assert_documented(&doc, 202, &accepted);
    let poll = accepted["poll"].as_str().expect("poll").to_string();
    let polled = loop {
        let r = http.get(&poll).send().await.expect("GET poll");
        if r.status() == 200 {
            break r.json::<Value>().await.expect("poll json");
        }
        assert_eq!(r.status(), 202);
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    assert_documented(&doc, 200, &polled);

    // 400: a body that is not JSON.
    let resp = http.post(&url).body("not json").send().await.expect("POST 400");
    assert_eq!(resp.status(), 400);
    let body: Value = resp.json().await.expect("400 json");
    assert_eq!(body["error"]["code"], json!("args_invalid"));
    assert_documented(&doc, 400, &body);

    // 500: the run ends in error.
    let resp = http.post(&url).body(r#"{"mode":"fail"}"#).send().await.expect("POST 500");
    assert_eq!(resp.status(), 500);
    assert_documented(&doc, 500, &resp.json().await.expect("500 json"));

    // 503: the admin kill switch pauses the tenant.
    let admin = McpClient::with_bearer(&base_url, "test-admin-key");
    admin
        .tools_call("admin.tenant_disable", json!({"tenant": tenant_ns}))
        .await
        .expect("admin.tenant_disable");
    let resp = http.post(&url).body(r#"{"mode":"ok"}"#).send().await.expect("POST 503");
    assert_eq!(resp.status(), 503);
    assert_documented(&doc, 503, &resp.json().await.expect("503 json"));

    // 404: an unknown token answers with an empty body; the document gives 404 no content.
    let unknown = url.replace(url.rsplit('/').nth(1).expect("token segment"), "no-such-token-0000000000");
    let resp = http.post(&unknown).body("{}").send().await.expect("POST 404");
    assert_eq!(resp.status(), 404);
    assert!(documented_validator(&doc, 404).is_none(), "404 is documented as an empty body");
    assert!(resp.bytes().await.expect("404 body").is_empty());

    // 429: past the per-IP minute budget (the limiter runs before the token lookup).
    let mut limited = None;
    for _ in 0..80 {
        let resp = http.post(&unknown).body("{}").send().await.expect("POST 429");
        if resp.status() == 429 {
            assert!(resp.headers().contains_key("retry-after"));
            limited = Some(resp.json::<Value>().await.expect("429 json"));
            break;
        }
    }
    assert_documented(&doc, 429, &limited.expect("no 429 within 80 calls"));
}

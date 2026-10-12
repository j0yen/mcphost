//! PRD-mcphost-call-cost-estimate-b
//! AC2 -- Given an anonymous caller, When it calls `host.estimate`, Then the
//! response is the same `auth_required` error the real call returns (no
//! `fits` field for unauthenticated callers).

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::{Value, json};

fn strip_request_id(mut data: Value) -> Value {
    if let Some(obj) = data.as_object_mut() {
        obj.remove("request_id");
    }
    data
}

/// Runs `host.tool_call` and `host.estimate` through the same client and
/// returns both outcomes.
async fn both(
    client: &McpClient,
    extra: Value,
) -> (Result<Value, common::RpcError>, Result<Value, common::RpcError>) {
    let mut real_args = json!({"name": "echoer", "args": {}});
    let mut est_args = json!({"name": "echoer", "args": {}});
    for args in [&mut real_args, &mut est_args] {
        if let (Some(a), Some(e)) = (args.as_object_mut(), extra.as_object()) {
            a.extend(e.clone());
        }
    }
    let real = client.tools_call("host.tool_call", real_args).await;
    let est = client.tools_call("host.estimate", est_args).await;
    (real, est)
}

#[tokio::test]
async fn unauthenticated_estimate_gets_the_error_the_real_call_gets() {
    let server = TestServer::start().await;
    let (ns, _key) = signup(&server.base_url, "Cest Path Owner").await;

    // The three ways a caller reaches the host unauthenticated and is
    // refused: a `tenant_key` that does not resolve, an `Authorization`
    // header that does not resolve, and no credential at all on a tenant's
    // own `/t/{ns}/mcp` path.
    let clients = [
        ("bad tenant_key", McpClient::new(&server.base_url), json!({"tenant_key": "t_notreal"})),
        ("bad bearer", McpClient::with_bearer(&server.base_url, "t_notreal"), json!({})),
        ("tenant path", McpClient::new(&server.base_url).with_path(&format!("/t/{ns}/mcp")), json!({})),
    ];
    for (label, client, extra) in clients {
        match both(&client, extra).await {
            (Err(real), Err(est)) => {
                assert!(real.error_code.is_some(), "{label}: the real call is refused with a coded error");
                assert_eq!(est.error_code, real.error_code, "{label}");
                assert_eq!(est.message, real.message, "{label}");
                assert_eq!(strip_request_id(est.data.clone()), strip_request_id(real.data.clone()), "{label}");
                assert!(est.data.get("fits").is_none(), "{label}: no verdict for an unauthenticated caller");
            }
            (real, est) => panic!("{label}: both must be refused; real={real:?} estimate={est:?}"),
        }
    }
}

/// A bare anonymous call on `/mcp` is not refused at all: the host signs the
/// caller up and runs the call as that new tenant (PRD-mcphost-implicit-signup).
/// `host.estimate` follows the real call there, so it answers for the new
/// tenant rather than inventing a second rule for anonymous callers.
#[tokio::test]
async fn a_bare_anonymous_estimate_follows_the_real_call_into_implicit_signup() {
    let server = TestServer::start().await;
    let anon = McpClient::new(&server.base_url);
    let (real, est) = both(&anon, json!({})).await;
    let real = real.expect_err("the signed-up tenant has no 'echoer', as for the real call");
    assert_eq!(real.error_code.as_deref(), Some("tool_not_found"));
    let est = common::extract_structured(&est.expect("an estimate answers for the signed-up tenant"));
    assert_eq!(est["dry_run"], json!(true), "{est}");
    assert_eq!(est["refusal"]["code"], json!("tool_not_found"), "{est}");
    assert!(est["onboarding"].is_object(), "the signup note rides along as it does for the real call: {est}");
}

#[tokio::test]
async fn a_keyed_tenant_gets_a_verdict_from_host_estimate() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Cest Keyed").await;
    let keyed = McpClient::with_bearer(&server.base_url, &key);
    let ok = keyed.tools_call("host.estimate", json!({"name": "nope", "args": {}})).await;
    let body = common::extract_structured(&ok.expect("a keyed estimate answers"));
    assert_eq!(body["dry_run"], json!(true), "{body}");
    assert!(body.get("fits").is_some(), "{body}");
}

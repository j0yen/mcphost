//! PRD-mcphost-blob-retention-and-quota
//! AC5 -- Given a free tenant, When `host.blob.put` succeeds and when it is
//! refused `quota_blob_bytes`, Then both responses carry
//! `data.quota.blob_bytes_max = {used, limit, remaining}` identical in shape
//! to `data.quota.table_bytes_max`, and the `dquota` drift test passes with
//! the three blob names in its list (tests/dquota_ac04_*.rs).

use crate::common::{McpClient, TestServer, extract_structured, signup};
use base64::Engine as _;
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn args(name: &str, fill: u8, len: usize) -> Value {
    json!({
        "name": name,
        "mime": "application/octet-stream",
        "content_base64": base64::engine::general_purpose::STANDARD.encode(vec![fill; len]),
    })
}

fn keys(v: &Value) -> BTreeSet<String> {
    v.as_object().expect("object").keys().cloned().collect()
}

#[tokio::test]
async fn put_and_its_quota_refusal_both_carry_blob_bytes_max_in_the_table_bytes_shape() {
    let server = TestServer::start_with_plans(|catalog| {
        for p in &mut catalog.plans {
            if p.name == "free" {
                p.blob_bytes_max = 100;
            }
        }
    })
    .await;
    let (_ns, key) = signup(&server.base_url, "Blobgc AC5").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let free = server.state.plans.get("free").expect("free").clone();

    let table = extract_structured(&client.tools_call("host.table.list", json!({})).await.expect("table.list"));
    let table_shape = keys(&table["data"]["quota"]["table_bytes_max"]);
    assert_eq!(table_shape, ["limit", "remaining", "used"].map(String::from).into_iter().collect());

    // Success: 60 of 100 bytes used.
    let ok = extract_structured(&client.tools_call("host.blob.put", args("a.bin", 1, 60)).await.expect("put a"));
    let quota = &ok["data"]["quota"];
    assert_eq!(quota["blob_bytes_max"], json!({"used": 60, "limit": 100, "remaining": 40}), "{ok:?}");
    assert_eq!(keys(&quota["blob_bytes_max"]), table_shape, "same shape as table_bytes_max");
    assert_eq!(quota["blobs_max"], json!({"used": 1, "limit": free.blobs_max, "remaining": free.blobs_max - 1}));
    // A per-file cap has no stored usage: used and remaining are null.
    assert_eq!(quota["blob_max_bytes"], json!({"used": null, "limit": free.blob_max_bytes, "remaining": null}));
    assert_eq!(keys(quota), ["blob_bytes_max", "blob_max_bytes", "blobs_max"].map(String::from).into_iter().collect());

    // Refusal: another 60 bytes would pass 100.
    let err = client
        .tools_call("host.blob.put", args("b.bin", 2, 60))
        .await
        .expect_err("past blob_bytes_max must refuse");
    assert_eq!(err.error_code.as_deref(), Some("quota_blob_bytes"), "{err:?}");
    assert_eq!(err.data["ceiling"], json!("blob_bytes_max"), "{:?}", err.data);
    assert_eq!(err.data["quota"]["blob_bytes_max"], json!({"used": 60, "limit": 100, "remaining": 0}), "{:?}", err.data);
    assert_eq!(keys(&err.data["quota"]["blob_bytes_max"]), table_shape);

    // Every other blob verb carries it too.
    for (verb, a) in [("host.blob.list", json!({})), ("host.blob.get", json!({"name": "a.bin"}))] {
        let body = extract_structured(&client.tools_call(verb, a).await.expect(verb));
        assert_eq!(body["data"]["quota"]["blob_bytes_max"]["used"], json!(60), "{verb}: {body:?}");
    }
    let missing = client.tools_call("host.blob.get", json!({"name": "nope"})).await.expect_err("not found");
    assert_eq!(missing.error_code.as_deref(), Some("blob_not_found"));
    assert_eq!(missing.data["quota"]["blob_bytes_max"]["limit"], json!(100), "{:?}", missing.data);

    // quickstart and usage name the same three ceilings.
    let usage = extract_structured(&client.tools_call("host.usage", json!({})).await.expect("usage"));
    assert_eq!(usage["quota"]["blob_bytes_max"], json!({"used": 60, "limit": 100, "remaining": 40}));
}

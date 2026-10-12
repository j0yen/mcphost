//! PRD-mcphost-blob-retention-and-quota
//! AC3 -- Given a tenant policy `keep_latest_versions: 2`, When the same name
//! is put four times, Then versions 1 and 2 are marked expired and
//! `host.blob.gc {dry_run: true}` lists exactly those two.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use base64::Engine as _;
use serde_json::{Value, json};

fn b64(s: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(s.as_bytes())
}

async fn put(client: &McpClient, name: &str, body: &str) -> Value {
    extract_structured(
        &client
            .tools_call("host.blob.put", json!({"name": name, "content_base64": b64(body)}))
            .await
            .expect("put"),
    )
}

#[tokio::test]
async fn four_puts_under_keep_2_expire_versions_1_and_2_and_gc_lists_them() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Blobgc AC3").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let policy = extract_structured(
        &client.tools_call("host.blob.policy", json!({"keep_latest_versions": 2})).await.expect("policy"),
    );
    assert_eq!(policy["keep_latest_versions"], json!(2));
    assert_eq!(policy["notify_before_s"], json!(0));

    let mut id = String::new();
    for (i, body) in ["one", "two", "three", "four"].iter().enumerate() {
        let out = put(&client, "report.txt", body).await;
        assert_eq!(out["version"], json!(i as i64 + 1));
        id = out["id"].as_str().unwrap().to_string();
        // Nothing is listed until a version falls outside the newest two.
        let gc = extract_structured(&client.tools_call("host.blob.gc", json!({})).await.expect("gc"));
        assert_eq!(gc["count"], json!((i as i64 - 1).max(0)), "after put {}: {gc:?}", i + 1);
    }

    let gc = extract_structured(&client.tools_call("host.blob.gc", json!({"dry_run": true})).await.expect("gc"));
    let listed: Vec<(i64, String, i64)> = gc["expiring"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["version"].as_i64().unwrap(), e["name"].as_str().unwrap().to_string(), e["bytes"].as_i64().unwrap()))
        .collect();
    assert_eq!(
        listed,
        vec![(1, "report.txt".to_string(), 3), (2, "report.txt".to_string(), 3)],
        "exactly versions 1 and 2: {gc:?}"
    );
    assert_eq!(gc["count"], json!(2));
    assert_eq!(gc["bytes_total"], json!(6));
    assert!(gc["expiring"].as_array().unwrap().iter().all(|e| e["id"] == json!(id)));

    // The current version is untouched and still serves the newest bytes.
    let got = extract_structured(
        &client.tools_call("host.blob.get", json!({"name": "report.txt", "as": "base64"})).await.expect("get"),
    );
    assert_eq!(got["content_base64"], json!(b64("four")));
    assert_eq!(got["version"], json!(4));

    // dry_run: false removes exactly that set, rows and files.
    let dir = server.state.db.data_dir().join("blobs");
    let files = |dir: &std::path::Path| -> usize {
        std::fs::read_dir(dir).map(|d| d.count()).unwrap_or(0)
    };
    let tenant_dir = std::fs::read_dir(&dir).unwrap().next().unwrap().unwrap().path();
    assert_eq!(files(&tenant_dir), 4, "current + versions 1-3 on disk before the purge");
    let purged = extract_structured(&client.tools_call("host.blob.gc", json!({"dry_run": false})).await.expect("gc"));
    assert_eq!(purged["deleted"], json!(2));
    assert_eq!(purged["bytes_freed"], json!(6));
    assert_eq!(files(&tenant_dir), 2, "current + version 3 remain");
    let after = extract_structured(&client.tools_call("host.blob.gc", json!({})).await.expect("gc"));
    assert_eq!(after["count"], json!(0));
}

#[tokio::test]
async fn without_a_policy_a_replaced_version_is_not_kept() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Blobgc AC3 plain").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    for body in ["a", "b", "c"] {
        put(&client, "n.txt", body).await;
    }
    let gc = extract_structured(&client.tools_call("host.blob.gc", json!({})).await.expect("gc"));
    assert_eq!(gc["count"], json!(0));
    let tenant_dir = std::fs::read_dir(server.state.db.data_dir().join("blobs")).unwrap().next().unwrap().unwrap().path();
    assert_eq!(std::fs::read_dir(tenant_dir).unwrap().count(), 1, "only the current file");
}

#[tokio::test]
async fn policy_validates_and_clears_fields() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Blobgc AC3 policy").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    for bad in [json!({"keep_latest_versions": 0}), json!({"keep_latest_versions": 101}), json!({"default_ttl_s": 0}), json!({"notify_before_s": -1})] {
        assert!(client.tools_call("host.blob.policy", bad.clone()).await.is_err(), "{bad}");
    }
    let mut set = extract_structured(
        &client
            .tools_call("host.blob.policy", json!({"default_ttl_s": 3600, "keep_latest_versions": 3, "notify_before_s": 60}))
            .await
            .unwrap(),
    );
    set.as_object_mut().unwrap().remove("data");
    assert_eq!(set, json!({"default_ttl_s": 3600, "keep_latest_versions": 3, "notify_before_s": 60}));
    let mut cleared = extract_structured(
        &client.tools_call("host.blob.policy", json!({"default_ttl_s": null})).await.unwrap(),
    );
    cleared.as_object_mut().unwrap().remove("data");
    assert_eq!(cleared, json!({"default_ttl_s": null, "keep_latest_versions": 3, "notify_before_s": 60}));

    // A put with no ttl_s takes the policy default.
    client.tools_call("host.blob.policy", json!({"default_ttl_s": 600})).await.unwrap();
    let out = put(&client, "d.txt", "x").await;
    let expires_at = out["expires_at"].as_i64().expect("default ttl applied");
    assert!((expires_at - mcphost::state::now_unix() - 600).abs() <= 5);
}

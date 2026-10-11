//! PRD-mcphost-blob-put-get-signed-url
//! AC7 — Given two tenants, When tenant B calls `host.blob.get` with tenant
//! A's id or fetches A's URL signed with B's key, Then both answer not found
//! / 403 and A's `host.blob.list` is unchanged.

use crate::common;
use base64::Engine as _;
use common::{McpClient, TestServer, extract_structured, signup};
use reqwest::StatusCode;
use serde_json::json;

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[tokio::test]
async fn tenant_b_cannot_get_or_fetch_tenant_a_blobs() {
    let server = TestServer::start().await;
    let (_ns_a, key_a) = signup(&server.base_url, "Blobput AC7 A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Blobput AC7 B").await;
    let a = McpClient::with_bearer(&server.base_url, &key_a);
    let b = McpClient::with_bearer(&server.base_url, &key_b);

    let put = extract_structured(
        &a.tools_call("host.blob.put", json!({"name": "secret.txt", "content_base64": b64(b"only for A")}))
            .await
            .expect("A put"),
    );
    let id = put["id"].as_str().unwrap().to_string();
    let a_url = put["url"].as_str().unwrap().to_string();
    let before = extract_structured(&a.tools_call("host.blob.list", json!({})).await.expect("A list"));
    assert_eq!(before["count"], 1);

    // B asks for A's id by every verb that takes one: not found, no bytes.
    for (verb, args) in [
        ("host.blob.get", json!({"id": id})),
        ("host.blob.get", json!({"id": id, "as": "base64"})),
        ("host.blob.url", json!({"id": id})),
        ("host.blob.delete", json!({"id": id})),
    ] {
        let err = b.tools_call(verb, args).await.expect_err("another tenant's id is not found");
        assert_eq!(err.data["error_code"], json!("blob_not_found"), "{verb}: {err:?}");
    }
    let by_name = b.tools_call("host.blob.get", json!({"name": "secret.txt"})).await.expect_err("B has no such name");
    assert_eq!(by_name.data["error_code"], json!("blob_not_found"));
    let b_list = extract_structured(&b.tools_call("host.blob.list", json!({})).await.expect("B list"));
    assert_eq!(b_list["count"], 0, "B sees none of A's blobs");

    // A URL for A's blob signed with B's key does not verify against A's key.
    let b_hash = server
        .state
        .db
        .find_tenant_by_namespace(ns_b)
        .await
        .expect("find B")
        .expect("B exists")
        .key_hash;
    let forged = mcphost::signed_url::mint(
        &server.base_url,
        &b_hash,
        &format!("/blobs/{id}"),
        "expires",
        mcphost::state::now_unix() + 600,
    );
    let resp = reqwest::get(&forged).await.expect("GET forged");
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "forged: {forged}");

    // A's own link still serves, and A's list did not change.
    let ok = reqwest::get(&a_url).await.expect("GET A's url");
    assert_eq!(ok.status(), StatusCode::OK);
    assert_eq!(ok.bytes().await.unwrap().as_ref(), b"only for A");
    let after = extract_structured(&a.tools_call("host.blob.list", json!({})).await.expect("A list again"));
    assert_eq!(after, before, "A's host.blob.list is unchanged");
}

#[tokio::test]
async fn get_list_delete_and_url_work_for_the_owner() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Blobput AC7 Owner").await;
    let c = McpClient::with_bearer(&server.base_url, &key);
    for (name, body) in [("docs/a.txt", &b"aaa"[..]), ("docs/b.txt", &b"bb"[..]), ("other.bin", &b"c"[..])] {
        c.tools_call("host.blob.put", json!({"name": name, "content_base64": b64(body)})).await.expect("put");
    }

    let listed = extract_structured(&c.tools_call("host.blob.list", json!({"prefix": "docs/"})).await.expect("list"));
    let names: Vec<&str> = listed["blobs"].as_array().unwrap().iter().map(|b| b["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["docs/a.txt", "docs/b.txt"]);
    assert_eq!(listed["count"], 3);
    assert_eq!(listed["total_bytes"], 6, "R8: total_bytes across the tenant's blobs");

    let got = extract_structured(
        &c.tools_call("host.blob.get", json!({"name": "docs/a.txt", "as": "base64"})).await.expect("get"),
    );
    assert_eq!(got["content_base64"], b64(b"aaa"));
    assert_eq!(got["mime"], "text/plain");
    let by_url = extract_structured(&c.tools_call("host.blob.get", json!({"name": "docs/a.txt"})).await.expect("get"));
    assert!(by_url.get("content_base64").is_none() && by_url["url"].as_str().is_some());

    let id = got["id"].as_str().unwrap();
    let minted = extract_structured(&c.tools_call("host.blob.url", json!({"id": id, "ttl_s": 60})).await.expect("url"));
    let resp = reqwest::get(minted["url"].as_str().unwrap()).await.unwrap();
    assert_eq!(resp.text().await.unwrap(), "aaa");
    let too_long = c.tools_call("host.blob.url", json!({"id": id, "ttl_s": 7 * 24 * 3600 + 1})).await;
    assert!(too_long.is_err(), "ttl_s over 7 days is refused");

    let deleted = extract_structured(&c.tools_call("host.blob.delete", json!({"id": id})).await.expect("delete"));
    assert_eq!(deleted["deleted"], true);
    assert_eq!(reqwest::get(minted["url"].as_str().unwrap()).await.unwrap().status(), StatusCode::NOT_FOUND);
    let gone = c.tools_call("host.blob.get", json!({"id": id})).await.expect_err("deleted");
    assert_eq!(gone.data["error_code"], json!("blob_not_found"));
}

#[tokio::test]
async fn base64_get_of_a_blob_over_1_mib_names_host_blob_url() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Blobput AC7 Inline").await;
    let c = McpClient::with_bearer(&server.base_url, &key);
    // Under the 1 MiB request cap once base64-framed, so put it in two
    // steps: 700 KB of zeros stays inline-able, then a 1 MiB + 1 file is
    // written through the library path (the /mcp body cap is 1 MiB).
    let small = extract_structured(
        &c.tools_call("host.blob.put", json!({"name": "s.bin", "content_base64": b64(&vec![0u8; 700 * 1024])}))
            .await
            .expect("put"),
    );
    c.tools_call("host.blob.get", json!({"id": small["id"], "as": "base64"})).await.expect("700 KB inline is fine");

    let (_ns2, key2) = signup(&server.base_url, "Blobput AC7 Inline Big").await;
    let tenant_row = server
        .state
        .db
        .find_tenant_by_key_hash(mcphost::auth::hash_key(&key2))
        .await
        .expect("lookup")
        .expect("tenant");
    let big = mcphost::blobs::blob_put(
        &server.state,
        &tenant_row,
        &json!({"name": "big.bin", "content_base64": b64(&vec![1u8; 1024 * 1024 + 1])}),
    )
    .await
    .expect("put big via the library");
    let c2 = McpClient::with_bearer(&server.base_url, &key2);
    let err = c2
        .tools_call("host.blob.get", json!({"id": big["id"], "as": "base64"}))
        .await
        .expect_err("over 1 MiB inline is refused");
    assert_eq!(err.data["error_code"], json!("blob_too_large_inline"));
    assert!(format!("{err:?}").contains("host.blob.url"), "{err:?}");
}

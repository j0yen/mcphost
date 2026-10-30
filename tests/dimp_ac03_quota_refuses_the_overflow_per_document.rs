//! PRD-mcphost-docs-import-and-stated-boundary
//! AC3 -- Given a tenant whose `docs_max` leaves room for 2 more documents,
//! When `put_many` is called with 5, Then `stored` is 2, the other 3 carry
//! `status` `refused` with `error_code` `quota_docs`, every stored document is
//! readable through `host.docs.get`, and the call's JSON-RPC result is a
//! success, not an error.

use crate::common;
use crate::dimp;

use common::{TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn dimp_ac03_a_batch_over_docs_max_stores_what_fits_and_refuses_the_rest() {
    let server = TestServer::start_with_plans(|catalog| {
        for plan in &mut catalog.plans {
            if plan.name == "free" {
                plan.docs_max = 3;
            }
        }
    })
    .await;
    let client = dimp::tenant(&server, "dimp ac3").await;
    client.tools_call("host.docs.put", json!({"name": "already.md", "content": "one doc in"})).await.expect("seed put");

    let reply = dimp::put_many(&client, dimp::documents(5), None).await;
    assert_eq!(reply["stored"], json!(2), "reply: {reply}");
    assert_eq!(reply["refused"], json!(3), "reply: {reply}");

    let results = reply["results"].as_array().expect("results");
    assert_eq!(results.len(), 5);
    for row in &results[..2] {
        assert_eq!(row["status"], json!("stored"), "row: {row}");
    }
    for row in &results[2..] {
        assert_eq!(row["status"], json!("refused"), "row: {row}");
        assert_eq!(row["error_code"], json!("quota_docs"), "row: {row}");
        assert!(row["message"].as_str().is_some_and(|m| m.contains("docs_max")), "row: {row}");
    }

    for name in ["doc-00.md", "doc-01.md"] {
        let got = client.tools_call("host.docs.get", json!({"name": name})).await.expect("a stored document reads back");
        assert_eq!(extract_structured(&got)["name"], json!(name));
    }
    for name in ["doc-02.md", "doc-03.md", "doc-04.md"] {
        assert!(client.tools_call("host.docs.get", json!({"name": name})).await.is_err(), "{name} was refused, not written");
    }
    assert_eq!(dimp::listed_names(&client).await.len(), 3);
}

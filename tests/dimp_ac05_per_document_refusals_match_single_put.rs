//! PRD-mcphost-docs-import-and-stated-boundary
//! AC5 -- Given `put_many` with one document over `MCPHOST_DOC_MAX_BYTES` and
//! one with invalid `content_base64` among three valid ones, When it runs, Then
//! the two carry `status` `refused` with the same `error_code` and `message` a
//! single `host.docs.put` of that document returns, the three are `stored`, and
//! the call's JSON-RPC result is a success (R2).

use crate::common;
use crate::dimp;

use common::{EnvGuard, TestServer};
use serde_json::{Value, json};

#[tokio::test]
async fn dimp_ac05_refused_rows_carry_the_single_put_error_code_and_message() {
    let _cap = EnvGuard::set("MCPHOST_DOC_MAX_BYTES", "100").await;
    let server = TestServer::start().await;
    let client = dimp::tenant(&server, "dimp ac5").await;

    let too_big = json!({"name": "big.md", "content": "x".repeat(300)});
    let bad_base64 = json!({"name": "bad.md", "content_base64": "!!!not base64!!!"});
    let documents: Vec<Value> = vec![
        json!({"name": "a.md", "content": "alpha"}),
        too_big.clone(),
        json!({"name": "b.md", "content": "beta"}),
        bad_base64.clone(),
        json!({"name": "c.md", "content": "gamma"}),
    ];

    let reply = dimp::put_many(&client, documents, None).await;
    assert_eq!(reply["stored"], json!(3), "reply: {reply}");
    assert_eq!(reply["refused"], json!(2), "reply: {reply}");
    let results = reply["results"].as_array().expect("results");
    let statuses: Vec<&str> = results.iter().map(|r| r["status"].as_str().expect("status")).collect();
    assert_eq!(statuses, ["stored", "refused", "stored", "refused", "stored"]);

    for (row, document) in [(&results[1], too_big), (&results[3], bad_base64)] {
        let single = client.tools_call("host.docs.put", document).await.expect_err("the single put refuses it");
        assert_eq!(Some(row["error_code"].as_str().expect("error_code")), single.error_code.as_deref(), "row: {row}");
        assert_eq!(row["message"].as_str().expect("message"), single.message, "row: {row}");
    }
    assert_eq!(results[1]["error_code"], json!("docs_too_large"));

    let mut listed = dimp::listed_names(&client).await;
    listed.sort();
    assert_eq!(listed, ["a.md", "b.md", "c.md"]);
}

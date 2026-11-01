//! PRD-mcphost-docs-import-and-stated-boundary
//! AC4 -- Given 51 documents, When `put_many` is called, Then the reply is
//! `-32602` and `host.docs.list` is unchanged (nothing written).

use crate::common;
use crate::dimp;

use common::TestServer;
use serde_json::json;

#[tokio::test]
async fn dimp_ac04_over_the_cap_is_invalid_params_and_writes_nothing() {
    let server = TestServer::start().await;
    let client = dimp::tenant(&server, "dimp ac4").await;
    client.tools_call("host.docs.put", json!({"name": "before.md", "content": "kept"})).await.expect("seed put");
    let before = dimp::listed_names(&client).await;
    assert_eq!(before, vec!["before.md".to_string()]);

    let err = dimp::put_many_raw(&client, dimp::documents(51), None).await.expect_err("51 documents is refused");
    assert_eq!(err.code, -32602, "error: {err:?}");

    assert_eq!(dimp::listed_names(&client).await, before, "nothing was written");

    // The cap itself is the schema's bound, not a handler-only check: an empty list is refused too.
    let empty = dimp::put_many_raw(&client, Vec::new(), None).await.expect_err("an empty batch is refused");
    assert_eq!(empty.code, -32602, "error: {empty:?}");
    assert_eq!(dimp::listed_names(&client).await, before);
}

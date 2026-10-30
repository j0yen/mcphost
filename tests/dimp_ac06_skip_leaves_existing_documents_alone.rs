//! PRD-mcphost-docs-import-and-stated-boundary
//! AC6 -- Given `put_many` with `on_conflict` `skip` and two documents whose
//! names already exist, When it runs, Then those two report `skipped`, the rest
//! `stored`, and the existing content is unchanged.

use crate::common;
use crate::dimp;

use common::TestServer;
use serde_json::json;

#[tokio::test]
async fn dimp_ac06_skip_reports_existing_names_and_keeps_their_content() {
    let server = TestServer::start().await;
    let client = dimp::tenant(&server, "dimp ac6").await;
    for name in ["doc-01.md", "doc-03.md"] {
        client.tools_call("host.docs.put", json!({"name": name, "content": "original"})).await.expect("seed put");
    }

    let reply = dimp::put_many(&client, dimp::documents(5), Some("skip")).await;
    assert_eq!(reply["stored"], json!(3), "reply: {reply}");
    let statuses: Vec<&str> = reply["results"].as_array().expect("results").iter().map(|r| r["status"].as_str().expect("status")).collect();
    assert_eq!(statuses, ["stored", "skipped", "stored", "skipped", "stored"]);

    for name in ["doc-01.md", "doc-03.md"] {
        assert_eq!(dimp::text_of(&client, name).await, "original", "{name} is unchanged");
        let doc = common::extract_structured(&client.tools_call("host.docs.get", json!({"name": name})).await.expect("get"));
        assert_eq!(doc["version"], json!(1), "{name} was not re-versioned");
    }

    // Without skip the same names are replaced: a new version, same call shape.
    let again = dimp::put_many(&client, dimp::documents(5), None).await;
    let statuses: Vec<&str> = again["results"].as_array().expect("results").iter().map(|r| r["status"].as_str().expect("status")).collect();
    assert_eq!(statuses, ["replaced", "replaced", "replaced", "replaced", "replaced"]);
    assert!(dimp::text_of(&client, "doc-01.md").await.contains("dimpword1"));
}

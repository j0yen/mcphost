//! PRD-mcphost-sync-source-registry-and-create AC4 -- Given `kind:
//! "excel_online"`, a `csv_url` ref of `http://10.0.0.1/x.csv`, and `kind:
//! "google_sheet"` on a tenant with no Google connection, When each is
//! created, Then the errors are `args_invalid` (message lists the five
//! kinds), `host_not_allowed`, and `connection_missing {provider: "google"}`,
//! and no `syncs` row exists.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use mcphost::sync::SourceKind;
use serde_json::json;

#[tokio::test]
async fn syreg_ac04_bad_sources_are_refused_and_leave_no_row() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "SYREG AC4").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.sync.create",
            json!({"source": {"kind": "excel_online", "ref": "x"}, "table": "t1"}),
        )
        .await
        .expect_err("unknown kind");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
    let message = format!("{err:?}");
    for kind in SourceKind::ALL {
        assert!(message.contains(kind.as_str()), "the message lists {}: {message}", kind.as_str());
    }

    let err = client
        .tools_call(
            "host.sync.create",
            json!({"source": {"kind": "csv_url", "ref": "http://10.0.0.1/x.csv"}, "table": "t2"}),
        )
        .await
        .expect_err("private host");
    assert_eq!(err.error_code.as_deref(), Some("host_not_allowed"), "{err:?}");

    let err = client
        .tools_call(
            "host.sync.create",
            json!({"source": {"kind": "google_sheet", "ref": "1AbCdEf"}, "table": "t3"}),
        )
        .await
        .expect_err("no google connection");
    assert_eq!(err.error_code.as_deref(), Some("connection_missing"), "{err:?}");
    assert_eq!(err.data["provider"], "google", "{:?}", err.data);

    let listed = extract_structured(&client.tools_call("host.sync.list", json!({})).await.expect("list"));
    assert_eq!(listed["syncs"].as_array().expect("syncs").len(), 0, "no syncs row exists: {listed}");
}

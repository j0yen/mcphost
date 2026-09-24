//! PRD-mcphost-admin-schema-contract
//! AC1 (P0) — Given a call to `admin.tenants`, When the response is read,
//! Then it carries integer `schema_version` = 1 and rows unchanged
//! otherwise.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn tenants_listing_carries_schema_version_one_rows_unchanged() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let (ns, _key) = signup(&server.base_url, "Schema Version Tenant").await;

    let result = admin
        .tools_call("admin.tenants", json!({}))
        .await
        .expect("admin.tenants");
    let listing = extract_structured(&result);

    assert_eq!(
        listing["schema_version"], json!(1),
        "admin.tenants must carry integer schema_version = 1: {listing:?}"
    );
    assert!(
        listing["schema_version"].is_i64() || listing["schema_version"].is_u64(),
        "schema_version must be an integer, not e.g. a string: {listing:?}"
    );

    let row = listing["tenants"]
        .as_array()
        .expect("tenants array")
        .iter()
        .find(|t| t["tenant"] == json!(ns))
        .unwrap_or_else(|| panic!("tenant {ns} not found in admin.tenants listing"));
    // Row shape is exactly the pre-existing fields (requirement 1: "rows
    // unchanged otherwise") -- schema_version is additive at the top level
    // only, never injected into a row.
    let mut fields: Vec<&str> = row.as_object().expect("row object").keys().map(String::as_str).collect();
    fields.sort_unstable();
    assert_eq!(
        fields,
        vec![
            "client_name",
            "client_version",
            "created_at",
            "disabled",
            "disabled_reason",
            "display_name",
            "owner_verified",
            "source_class",
            "synthetic",
            "tenant",
        ],
        "admin.tenants row fields changed: {row:?}"
    );
}

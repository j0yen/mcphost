//! PRD-mcphost-tool-naming-convention-and-aliases
//! AC3 — Given `tools/list`, When fetched, Then every canonical name is
//! present, every alias is present with `x-deprecated.replaced_by` equal
//! to its canonical and a `sunset` date 90 days after landing, and the
//! alias's `inputSchema` equals the canonical's.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::tool_aliases::{SUNSET_DATE, TOOL_ALIASES};
use serde_json::Value;
use std::collections::HashMap;

#[tokio::test]
async fn every_alias_is_listed_with_x_deprecated_and_the_canonicals_schema() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let listed = client.tools_list().await.expect("tools/list");
    let tools: Vec<&Value> = listed["tools"].as_array().expect("tools array").iter().collect();
    let by_name: HashMap<&str, &Value> =
        tools.iter().map(|t| (t["name"].as_str().expect("name"), *t)).collect();

    for a in TOOL_ALIASES {
        let canonical =
            by_name.get(a.canonical).unwrap_or_else(|| panic!("canonical {} must be in tools/list", a.canonical));
        let alias =
            by_name.get(a.alias).unwrap_or_else(|| panic!("alias {} must be in tools/list", a.alias));

        let x_deprecated = alias["inputSchema"]["x-deprecated"].clone();
        assert_eq!(
            x_deprecated["replaced_by"], a.canonical,
            "{}'s x-deprecated.replaced_by must be its canonical: {x_deprecated}",
            a.alias
        );
        assert_eq!(
            x_deprecated["sunset"], SUNSET_DATE,
            "{}'s x-deprecated.sunset must be the landing + 90 days date: {x_deprecated}",
            a.alias
        );
        assert!(
            canonical["inputSchema"]["x-deprecated"].is_null(),
            "the canonical {} must carry no x-deprecated of its own: {canonical}",
            a.canonical
        );

        // The alias's inputSchema must equal the canonical's except for
        // the one additive x-deprecated key this PRD adds.
        let mut alias_schema = alias["inputSchema"].clone();
        alias_schema.as_object_mut().expect("object").remove("x-deprecated");
        assert_eq!(
            alias_schema, canonical["inputSchema"],
            "{}'s inputSchema (minus x-deprecated) must equal {}'s",
            a.alias, a.canonical
        );
    }
}

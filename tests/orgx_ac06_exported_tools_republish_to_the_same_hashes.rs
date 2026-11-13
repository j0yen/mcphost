//! PRD-mcphost-org-export-and-offboard-v2
//! AC6 — Given a fixture org, When every tool in each member part's
//! `manifest.json` is re-published into a fresh org with `host.tool_publish`
//! and the fresh org is exported, Then the two `org_manifest.json` files agree
//! on member count and, member by member, on `tools[].name`, `kind` and
//! `spec_sha256`.

use crate::common;
use common::TestServer;
use common::orgx::{OrgFixture, download_bundle, fixture_org, read_tar_gz, run_org_export};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// The org manifest and each member part's `manifest.json` `tools`, member by member.
async fn export_of(fx: &OrgFixture) -> (Value, Vec<Vec<Value>>) {
    let result = run_org_export(&fx.owner).await;
    let bundle = download_bundle(&result).await;
    let manifest: Value = serde_json::from_slice(&bundle["org_manifest.json"]).expect("org_manifest.json");
    let parts = manifest["members"]
        .as_array()
        .expect("members")
        .iter()
        .map(|row| {
            let entries = read_tar_gz(&bundle[row["part"].as_str().expect("part")]);
            let inner: Value = serde_json::from_slice(&entries["manifest.json"]).expect("part manifest.json");
            inner["tools"].as_array().expect("tools").clone()
        })
        .collect();
    (manifest, parts)
}

/// `(name, kind, spec_sha256)` of every tool row, sorted by name.
fn tool_rows(member: &Value) -> Vec<(String, String, String)> {
    let mut rows: Vec<_> = member["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|t| (t["name"].as_str().unwrap().to_string(), t["kind"].as_str().unwrap().to_string(), t["spec_sha256"].as_str().unwrap().to_string()))
        .collect();
    rows.sort();
    rows
}

#[tokio::test]
async fn orgx_ac06() {
    let server = TestServer::start_with_signup_rate_limit(100).await;
    let original = fixture_org(&server, "acme", 2, 2).await;
    let (first, parts) = export_of(&original).await;

    // The recorded hash is the hash of the spec as the part's own manifest.json carries it.
    for (member, tools) in first["members"].as_array().unwrap().iter().zip(&parts) {
        let hashes: Vec<String> = tools.iter().map(|t| format!("{:x}", Sha256::digest(serde_json::to_vec(&t["spec"]).unwrap()))).collect();
        let mut recorded: Vec<String> = tool_rows(member).into_iter().map(|(_, _, h)| h).collect();
        let mut expected = hashes;
        recorded.sort();
        expected.sort();
        assert_eq!(recorded, expected, "spec_sha256 is over the spec as the part carries it");
    }

    // A fresh org of the same shape, empty; each member gets back its counterpart's tools,
    // every manifest entry passed as-is as the host.tool_publish arguments.
    let fresh = fixture_org(&server, "beta", 2, 0).await;
    let clients = std::iter::once(&fresh.owner).chain(fresh.members.iter().map(|(_, c)| c));
    for (client, tools) in clients.zip(&parts) {
        assert_eq!(tools.len(), 2);
        for entry in tools {
            client.tools_call("host.tool_publish", entry.clone()).await.unwrap_or_else(|e| panic!("republish {entry}: {} {}", e.code, e.message));
        }
    }
    let (second, _) = export_of(&fresh).await;

    let (a, b) = (first["members"].as_array().unwrap(), second["members"].as_array().unwrap());
    assert_eq!(a.len(), 3);
    assert_eq!(a.len(), b.len(), "member count agrees");
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        assert_eq!(tool_rows(x), tool_rows(y), "member {i}: tools[].name, kind and spec_sha256 agree");
        assert_eq!(tool_rows(x).len(), 2);
    }
    assert_eq!(json!(first["schema_version"]), json!(second["schema_version"]));
}

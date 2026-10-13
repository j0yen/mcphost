//! PRD-mcphost-host-changelog-since AC4 — Given a tenant whose
//! `last_seen_version` is 0.105.0 on a host at 0.108.0, When
//! `host.changelog {}` runs, Then `since` is `"0.105.0"`, `releases` are
//! exactly the headings in (0.105.0, 0.108.0], and `added` lists only tools
//! with `since` in that range.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::api_contract::{COMMITTED_CONTRACT, Version, changelog_at, parse_changelog, since_map};
use mcphost::kinds::KindRegistry;
use serde_json::json;

const FAKE_CHANGELOG: &str = "# Changelog\n\n\
## v0.109.0 — 2026-10-20\n\nnot yet released\n\n\
## v0.108.0 — 2026-10-14\n\neight\n\n\
## v0.107.0 — 2026-10-12\n\nseven\n\n\
## v0.106.0 — 2026-10-10\n\nsix\n\n\
## v0.105.0 — 2026-10-08\n\nfive\n\n\
## v0.104.0 — 2026-10-06\n\nfour\n";

#[test]
fn host_at_0_108_0_with_tenant_at_0_105_0_lists_exactly_the_range() {
    let releases = parse_changelog(FAKE_CHANGELOG).unwrap();
    let out = changelog_at(&KindRegistry::with_builtin(), &[], &releases, "0.108.0", Some("0.105.0"));
    assert_eq!(out["since"], "0.105.0");
    assert_eq!(out["now"], "0.108.0");
    let versions: Vec<&str> = out["releases"].as_array().unwrap().iter().map(|r| r["version"].as_str().unwrap()).collect();
    assert_eq!(versions, ["0.108.0", "0.107.0", "0.106.0"]);

    let (lo, hi) = (Version::parse("0.105.0").unwrap(), Version::parse("0.108.0").unwrap());
    let expected: std::collections::BTreeSet<String> = since_map(COMMITTED_CONTRACT)
        .into_iter()
        .filter(|(_, s)| {
            let v = Version::parse(s).unwrap();
            v > lo && v <= hi
        })
        .map(|(n, _)| n)
        .collect();
    let added: std::collections::BTreeSet<String> =
        out["added"].as_array().unwrap().iter().map(|a| a["name"].as_str().unwrap().to_string()).collect();
    assert_eq!(added, expected);
}

#[test]
fn a_never_seen_tenant_gets_since_null_and_the_last_five_releases() {
    let releases = parse_changelog(FAKE_CHANGELOG).unwrap();
    let out = changelog_at(&KindRegistry::with_builtin(), &[], &releases, "0.108.0", None);
    assert!(out["since"].is_null());
    assert_eq!(out["releases"].as_array().unwrap().len(), 5, "0.108.0 down to 0.104.0");
}

#[tokio::test]
async fn empty_args_use_the_version_the_tenant_last_called_with() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "CHG AC4").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // Stand the tenant at an older version; the next call loads that value
    // before it bumps the column to the host's own.
    let conn = rusqlite::Connection::open(server.state.db.data_dir().join("mcphost.db")).unwrap();
    conn.execute("UPDATE tenants SET last_seen_version = '0.107.1' WHERE namespace = ?1", [&ns]).unwrap();

    let out = extract_structured(&client.tools_call("host.changelog", json!({})).await.expect("changelog"));
    assert_eq!(out["since"], "0.107.1");
    let versions: Vec<Version> =
        out["releases"].as_array().unwrap().iter().map(|r| Version::parse(r["version"].as_str().unwrap()).unwrap()).collect();
    assert!(!versions.is_empty());
    assert!(versions.iter().all(|v| *v > Version::parse("0.107.1").unwrap() && *v <= Version::parse(env!("CARGO_PKG_VERSION")).unwrap()));

    // The call above bumped the column: the next default is "now".
    let again = extract_structured(&client.tools_call("host.changelog", json!({})).await.expect("changelog"));
    assert_eq!(again["since"], env!("CARGO_PKG_VERSION"));
    assert!(again["releases"].as_array().unwrap().is_empty());
}

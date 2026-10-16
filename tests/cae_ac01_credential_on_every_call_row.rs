//! PRD-mcphost-call-audit-export
//! contract: walks src/
//! AC1 (P0) — Given calls made with the primary key, a sub-key, an OAuth
//! token, a trigger fire, a cron tick, a webhook and a shared-caller sync
//! call, When `host.calls.list` runs, Then each row's `credential` is the
//! matching enum value and the sub-key row carries its `key_label`; the
//! write-path test lists ≥ 7 `INSERT INTO calls` sites and every one sets
//! `credential`.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use mcphost::calllog::{Credential, scoped};
use serde_json::{Value, json};
use std::path::Path;

async fn record(server: &TestServer, tenant_id: i64, tool: &str, auth_method: &str, caller: Option<i64>) {
    server
        .state
        .db
        .record_call_traced(
            tenant_id,
            tool.to_string(),
            3,
            true,
            None,
            None,
            None,
            "ok",
            "external".to_string(),
            None,
            caller,
            None,
            None,
            None,
            None,
            auth_method.to_string(),
            None,
            None,
            None,
            None,
        )
        .await
        .expect("record call");
}

fn rows_by_tool(listing: &Value) -> std::collections::BTreeMap<String, Value> {
    listing["calls"]
        .as_array()
        .expect("calls")
        .iter()
        .map(|r| (r["tool"].as_str().expect("tool").to_string(), r.clone()))
        .collect()
}

#[tokio::test]
async fn cae_ac01_every_credential_kind_is_stamped_and_listed() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Credential Tenant").await;
    let primary = McpClient::with_bearer(&server.base_url, &key);
    for name in ["via_primary", "via_subkey"] {
        primary
            .tools_call(
                "host.tool.publish",
                json!({"name": name, "kind": "echo", "spec": {"schema": {"type": "object"}}}),
            )
            .await
            .expect("publish");
    }
    let sub = extract_structured(
        &primary
            .tools_call("host.key.mint", json!({"tools": ["via_subkey"], "label": "sam-ci"}))
            .await
            .expect("mint"),
    )["key"]
        .as_str()
        .expect("key")
        .to_string();

    // Real HTTP: the primary key and the sub-key.
    primary.tools_call(&format!("{ns}.via_primary"), json!({})).await.expect("primary call");
    McpClient::with_bearer(&server.base_url, &sub)
        .tools_call(&format!("{ns}.via_subkey"), json!({}))
        .await
        .expect("sub-key call");

    // The write path itself, for the credentials a test cannot mint over
    // HTTP: an OAuth token (auth_method `hosted_token` / `issuer_jwt`), a
    // cross-tenant caller, and the scoped trigger / cron / webhook writers.
    let tenant = server.state.db.find_tenant_by_namespace(ns.clone()).await.expect("find").expect("tenant");
    record(&server, tenant.id, "via_oauth", "hosted_token", None).await;
    record(&server, tenant.id, "via_oauth_jwt", "issuer_jwt", None).await;
    let (caller_ns, _) = signup(&server.base_url, "Shared Caller Tenant").await;
    let caller = server.state.db.find_tenant_by_namespace(caller_ns).await.expect("find").expect("caller");
    record(&server, tenant.id, "via_shared", "key", Some(caller.id)).await;
    for (tool, cred) in [
        ("via_trigger", Credential::Trigger),
        ("via_cron", Credential::Cron),
        ("via_webhook", Credential::Webhook),
    ] {
        scoped(cred, record(&server, tenant.id, tool, "key", None)).await;
    }

    let listing = extract_structured(
        &primary.tools_call("host.calls.list", json!({"since": "-1h"})).await.expect("host.calls.list"),
    );
    let rows = rows_by_tool(&listing);
    let credential = |tool: &str| rows[tool]["credential"].as_str().expect("credential").to_string();
    assert_eq!(credential("via_primary"), "primary", "{listing}");
    assert_eq!(credential("via_subkey"), "sub_key", "{listing}");
    assert_eq!(rows["via_subkey"]["key_label"], json!("sam-ci"), "{listing}");
    assert_eq!(credential("via_oauth"), "oauth");
    assert_eq!(credential("via_oauth_jwt"), "oauth");
    assert_eq!(credential("via_shared"), "shared_caller");
    assert_eq!(credential("via_trigger"), "trigger");
    assert_eq!(credential("via_cron"), "cron");
    assert_eq!(credential("via_webhook"), "webhook");
    assert!(rows["via_primary"]["key_label"].is_null(), "only a sub-key row has a label");

    // Every stored value is a member of the one enum.
    let valid: Vec<&str> = Credential::ALL.iter().map(|c| c.as_str()).collect();
    for row in listing["calls"].as_array().expect("calls") {
        assert!(valid.contains(&row["credential"].as_str().expect("credential")), "{row}");
    }

    // The `credential` filter narrows to one kind.
    let only_cron = extract_structured(
        &primary
            .tools_call("host.calls.list", json!({"since": "-1h", "credential": "cron"}))
            .await
            .expect("filtered list"),
    );
    let tools: Vec<&str> = only_cron["calls"].as_array().expect("calls").iter().map(|r| r["tool"].as_str().expect("tool")).collect();
    assert_eq!(tools, vec!["via_cron"]);
}

/// The write-path test: every `INSERT INTO calls` in `src/` names
/// `credential` between the table and its `VALUES`, so a new write path that
/// forgets the column fails here rather than silently reading as `primary`.
#[test]
fn cae_ac01_every_insert_into_calls_sets_credential() {
    fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read_dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
    let mut sites = 0;
    let mut missing = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).expect("read source");
        for (at, _) in text.match_indices("INSERT INTO calls") {
            // Skip prose mentions in comments and doc strings.
            let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
            if text[line_start..at].trim_start().starts_with("//") {
                continue;
            }
            sites += 1;
            let statement = &text[at..];
            let upto = statement.find("VALUES").expect("INSERT INTO calls without VALUES");
            if !statement[..upto].contains("credential") {
                missing.push(format!("{}: {}", file.display(), &statement[..upto.min(80)]));
            }
        }
    }
    assert!(sites >= 7, "expected at least 7 `INSERT INTO calls` sites, found {sites}");
    assert!(missing.is_empty(), "INSERT INTO calls sites without `credential`: {missing:#?}");
}

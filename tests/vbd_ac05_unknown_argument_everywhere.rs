//! contract: an undeclared top-level key is refused unknown_argument from the one dispatch-level call site, with generated accepted_by and did_you_mean
//! PRD-mcphost-host-args-validated-by-default
//! AC5 (P0) -- Given every `host.*` and `admin.*` descriptor, When a call carries
//! one top-level key absent from its `properties`, Then `unknown_argument` from
//! the one dispatch-level call site with `data.known` (the properties),
//! `data.accepted_by` (verbs whose properties include the key, from the same
//! descriptors), `data.did_you_mean` (nearest declared property, null beyond 3
//! edits), and no other `check_unknown_argument(` caller in `src/`.

use crate::common;
use crate::vbd_calls;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use serde_json::{Value, json};

#[tokio::test]
async fn vbd_ac05_every_descriptor_refuses_an_undeclared_key_with_a_generated_remedy() {
    let server = TestServer::start_with_signup_rate_limit(100_000).await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let (_ns, key) = signup(&server.base_url, "VBD AC5").await;
    let tenant = McpClient::with_bearer(&server.base_url, &key);
    let mut offenders = Vec::new();
    let tools = vbd_calls::all_descriptors();
    for tool in &tools {
        let name = tool.name.to_string();
        let client = if vbd_calls::is_admin(tool) { &admin } else { &tenant };
        let mut args = vbd_calls::call_for(&tool.input_schema);
        args.as_object_mut().expect("object").insert("zzzzzzzzzzzz".into(), json!(1));
        let declared: Vec<&String> = tool.input_schema["properties"].as_object().expect("properties").keys().collect();
        match client.tools_call(&name, args).await {
            Err(e) if e.error_code.as_deref() == Some("unknown_argument") => {
                let known: Vec<&str> = e.data["known"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                if known.len() != declared.len() || !declared.iter().all(|d| known.contains(&d.as_str())) {
                    offenders.push(format!("{name}: data.known {known:?} != properties {declared:?}"));
                }
                if !e.data["accepted_by"].as_array().is_some_and(Vec::is_empty) {
                    offenders.push(format!("{name}: accepted_by for a key no verb declares: {}", e.data["accepted_by"]));
                }
                if !e.data["did_you_mean"].is_null() {
                    offenders.push(format!("{name}: did_you_mean for a key far from every property: {}", e.data["did_you_mean"]));
                }
            }
            other => offenders.push(format!("{name}: expected unknown_argument, got {other:?}")),
        }
    }
    assert!(tools.len() > 280, "only {} descriptors", tools.len());
    assert!(offenders.is_empty(), "{} offenders:\n{}", offenders.len(), offenders.join("\n"));
}

#[tokio::test]
async fn vbd_ac05_accepted_by_names_the_verb_that_declares_the_key_and_did_you_mean_the_nearest_property() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "VBD AC5 remedy").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call("host.tool_call", json!({"name": "x", "args": {}, "visibility": "public"}))
        .await
        .expect_err("a publish-style key on host.tool_call must be refused");
    assert_eq!(err.error_code.as_deref(), Some("unknown_argument"), "{err:?}");
    // Generated from the descriptors, not asserted as prose: every canonical verb
    // (not an alias clone) whose published properties include the key.
    let mut declaring: Vec<String> = vbd_calls::all_descriptors()
        .into_iter()
        .filter(|t| t.meta.as_ref().is_none_or(|m| !m.0.contains_key("alias_of")))
        .filter(|t| t.name != "host.tool.call" && t.input_schema["properties"].get("visibility").is_some())
        .map(|t| t.name.to_string())
        .collect();
    declaring.sort();
    assert_eq!(err.data["accepted_by"], json!(declaring), "{:?}", err.data);
    assert!(declaring.contains(&"host.tool.publish".to_string()), "{declaring:?}");
    assert!(err.data["known"].as_array().is_some_and(|k| k.contains(&json!("name"))), "{:?}", err.data);

    let err = client
        .tools_call("host.example.list", json!({"usecase": "x"}))
        .await
        .expect_err("a mistyped key must be refused");
    assert_eq!(err.error_code.as_deref(), Some("unknown_argument"), "{err:?}");
    assert_eq!(err.data["did_you_mean"], json!("use_case"), "{:?}", err.data);

    let err = client
        .tools_call("host.example.list", json!({"qqqqqqqqqqqqqqq": "x"}))
        .await
        .expect_err("a far key must be refused");
    assert!(err.data["did_you_mean"].is_null(), "{:?}", err.data);
}

#[test]
fn vbd_ac05_one_call_site_for_check_unknown_argument() {
    let mut sites = Vec::new();
    let mut stack = vec![std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/src"))];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(dir).expect("read src dir").flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                for (n, line) in text.lines().enumerate() {
                    let code = line.trim_start();
                    if code.starts_with("//") || code.contains("fn check_unknown_argument(") {
                        continue;
                    }
                    if code.contains("check_unknown_argument(") {
                        sites.push(format!("{}:{}", path.strip_prefix(env!("CARGO_MANIFEST_DIR")).unwrap().display(), n + 1));
                    }
                }
            }
        }
    }
    assert_eq!(sites.len(), 1, "check_unknown_argument( callers in src/: {sites:?}");
    assert!(sites[0].starts_with("src/host_args.rs"), "{sites:?}");
}

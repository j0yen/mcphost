//! AC7 (PRD-mcphost-plugin-skill-generated-from-tools-list) -- Given `--url
//! https://mcphost.dev/mcp --section verbs`, When rendered, Then the output is
//! derived from that host's `tools/list` and differs from the registry render
//! only by verbs not yet deployed.
//!
//! The host here is a local endpoint serving the registry's own descriptors
//! minus one verb (the "not yet deployed" one), paged in two to exercise the
//! cursor.

use std::collections::BTreeSet;

use axum::{Json, Router, routing::post};
use mcphost::kinds::KindRegistry;
use mcphost::skill_gen::{self, Section, Sources};
use serde_json::{Value, json};

const UNDEPLOYED: &str = "host.whoami";

fn deployed_tools() -> Vec<Value> {
    mcphost::handler::host_tool_descriptors(&KindRegistry::with_builtin())
        .into_iter()
        // The undeployed verb is absent under its dotted name and its flattened clone.
        .filter(|t| t.name != UNDEPLOYED && t.name != "host_whoami")
        .map(|t| serde_json::to_value(t).expect("tool serializes"))
        .collect()
}

async fn tools_list(Json(req): Json<Value>) -> Json<Value> {
    let tools = deployed_tools();
    let half = tools.len() / 2;
    let (page, next) = match req["params"]["cursor"].as_str() {
        None => (tools[..half].to_vec(), Some("page-2")),
        Some(_) => (tools[half..].to_vec(), None),
    };
    let mut result = json!({"tools": page});
    if let Some(n) = next {
        result["nextCursor"] = json!(n);
    }
    Json(json!({"jsonrpc": "2.0", "id": req["id"], "result": result}))
}

fn lines(text: &str) -> BTreeSet<String> {
    text.lines().map(str::to_string).collect()
}

#[tokio::test]
async fn skgn_ac07() {
    let app = Router::new().route("/mcp", post(tools_list));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://{}/mcp", listener.local_addr().expect("addr"));
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    // Library path: rows from the live list, rendered as the verbs section.
    let live = skill_gen::fetch_tools_list(&url, None).await.expect("fetch tools/list");
    assert!(live.iter().all(|r| r.name != UNDEPLOYED), "{UNDEPLOYED} is not deployed there");
    let from_url = skill_gen::render_verbs(&live);
    let registry = Sources::from_registry(&KindRegistry::with_builtin());
    let from_registry = skill_gen::render_section(Section::Verbs, &registry);

    let only_in_registry: Vec<String> = lines(&from_registry).difference(&lines(&from_url)).cloned().collect();
    let only_in_url: Vec<String> = lines(&from_url).difference(&lines(&from_registry)).cloned().collect();
    assert!(only_in_url.is_empty(), "the live render adds nothing the registry lacks: {only_in_url:?}");
    assert_eq!(only_in_registry.len(), 1, "differs by exactly the undeployed verb: {only_in_registry:?}");
    assert!(only_in_registry[0].starts_with(&format!("- `{UNDEPLOYED}` -- ")), "{only_in_registry:?}");

    // CLI path: `skill-gen --section verbs --url <url>` prints the plugin file
    // with the verbs block rendered from that host, and exits 0.
    let bin = env!("CARGO_BIN_EXE_mcphost");
    let out = tokio::task::spawn_blocking(move || {
        std::process::Command::new(bin)
            .args(["skill-gen", "--section", "verbs", "--url", &url])
            .env_remove("MCPHOST_KEY")
            .output()
            .expect("run mcphost skill-gen --url")
    })
    .await
    .expect("join");
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let start = stdout.find(&Section::Verbs.start_marker()).expect("verbs start marker in output");
    let end = stdout.find(&Section::Verbs.end_marker()).expect("verbs end marker in output") + Section::Verbs.end_marker().len();
    assert_eq!(&stdout[start..end], from_url, "the CLI block is the live render");
    assert!(!stdout.contains(&format!("- `{UNDEPLOYED}` -- ")));
}

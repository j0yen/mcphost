//! contract: reads registry handler::host_tool_descriptors
//! PRD-mcphost-notify-user-channel
//! AC1 -- Given `src/alerts.rs` at the cited tree, When a test enumerates
//! every sink source (`from_env`, any `alerts_*` table column, any `host.*`
//! verb that writes one), Then it records in its output that no sink is
//! tenant-settable (if one is, the PRD is parked and `alerts::raise` is
//! extended instead).

use crate::common;
use common::TempDataDir;
use mcphost::db::Db;
use serde_json::Value;

const SINK_WORDS: &[&str] = &["url", "webhook", "sink", "endpoint", "target", "email"];

fn is_sinkish(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    SINK_WORDS.iter().any(|w| lower.contains(w))
}

/// Every `std::env::var("...")` name `alerts.rs` reads.
fn env_vars_read_by_alerts() -> Vec<String> {
    let src = include_str!("../src/alerts.rs");
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(i) = rest.find("std::env::var(\"") {
        rest = &rest[i + "std::env::var(\"".len()..];
        if let Some(end) = rest.find('"') {
            out.push(rest[..end].to_string());
        }
    }
    out
}

#[tokio::test]
async fn ntfy_ac01() {
    let mut tenant_settable: Vec<String> = Vec::new();

    // Source 1: `AlertConfig::from_env` -- process env, set by the operator.
    let vars = env_vars_read_by_alerts();
    assert!(vars.iter().any(|v| v == "MCPHOST_ALERT_WEBHOOK_URL"), "{vars:?}");
    for v in &vars {
        println!("sink source env: {v} (operator process env)");
        if !v.starts_with("MCPHOST_ALERT_") {
            tenant_settable.push(format!("env {v}"));
        }
    }

    // Source 2: every column of every `alert*` table.
    let dir = TempDataDir::new();
    let db = Db::open(&dir.0).expect("open db");
    db.migrate().await.expect("migrate");
    let conn = rusqlite::Connection::open(dir.0.join("mcphost.db")).expect("raw db");
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name LIKE 'alert%' ORDER BY name")
        .expect("list tables")
        .query_map([], |r| r.get::<_, String>(0))
        .expect("tables")
        .collect::<Result<_, _>>()
        .expect("table names");
    assert!(tables.iter().any(|t| t == "alerts"), "{tables:?}");
    for t in &tables {
        let cols: Vec<String> = conn
            .prepare(&format!("PRAGMA table_info({t})"))
            .expect("table_info")
            .query_map([], |r| r.get::<_, String>(1))
            .expect("cols")
            .collect::<Result<_, _>>()
            .expect("col names");
        println!("sink source table: {t} columns {cols:?}");
        for c in cols.iter().filter(|c| is_sinkish(c)) {
            tenant_settable.push(format!("column {t}.{c}"));
        }
    }

    // Source 3: any tenant-facing `host.*` verb that names alerts or takes a
    // sink-shaped argument while doing so.
    let kinds = common::chain_kind_registry();
    for tool in mcphost::handler::host_tool_descriptors(&kinds) {
        let name = tool.name.to_string();
        if !name.contains("alert") {
            continue;
        }
        let props = tool.input_schema.get("properties").and_then(Value::as_object);
        println!("sink source verb: {name}");
        for p in props.into_iter().flat_map(|p| p.keys()).filter(|k| is_sinkish(k)) {
            tenant_settable.push(format!("verb {name}({p})"));
        }
        if name.starts_with("host.") {
            tenant_settable.push(format!("verb {name} is tenant-facing"));
        }
    }

    // `raise` itself takes no sink: its input is key/severity/title/body.
    let src = include_str!("../src/alerts.rs");
    let input = src.split("pub struct RaiseInput {").nth(1).and_then(|s| s.split('}').next()).expect("RaiseInput");
    for field in input.lines().filter_map(|l| l.trim().strip_prefix("pub ")) {
        let field = field.split(':').next().unwrap_or("");
        if is_sinkish(field) {
            tenant_settable.push(format!("RaiseInput.{field}"));
        }
    }

    assert!(tenant_settable.is_empty(), "a tenant can set an alert sink: {tenant_settable:?}");
    println!("RESULT: no alert sink is tenant-settable (every sink source is operator env or operator-only)");
}

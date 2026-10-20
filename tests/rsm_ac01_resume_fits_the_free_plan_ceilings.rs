//! PRD-mcphost-resume-tool AC1 -- Given a tenant seeded at the free plan's
//! ceilings (50 tools each with a failed last call, 1000 `notes/` rows of
//! 4 KB, 10 open runs, `last_seen_version` 20 releases back), When
//! `host.resume` is called, Then the response is <= 4096 bytes, `truncated`
//! names every cut section, and the unit test over `ResumeBudget`'s worst
//! case passes (the falsifier).

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::api_contract::{CHANGELOG_MD, parse_changelog};
use mcphost::resume::{BUDGET, ResumeBudget};
use serde_json::json;

fn seed_ceiling_tenant(server: &TestServer, namespace: &str, since: &str) {
    let conn = rusqlite::Connection::open(server.state.db.data_dir().join("mcphost.db")).unwrap();
    let tenant_id: i64 = conn
        .query_row("SELECT id FROM tenants WHERE namespace = ?1", [namespace], |r| r.get(0))
        .unwrap();
    conn.execute("UPDATE tenants SET last_seen_version = ?1 WHERE id = ?2", rusqlite::params![since, tenant_id]).unwrap();
    let now = 1_760_000_000_i64;
    for i in 0..50 {
        let name = format!("{}{i:02}", "t".repeat(39));
        conn.execute(
            "INSERT INTO tools (tenant_id, name, kind, spec, created_at) VALUES (?1, ?2, 'echo', '{}', 'unix:1.0')",
            rusqlite::params![tenant_id, name],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO calls (tenant_id, tool_name, started_at, started_unix, duration_ms, ok, error_class) \
             VALUES (?1, ?2, 'unix:1.0', ?3, 5, 0, 'tool_exception')",
            rusqlite::params![tenant_id, name, now + i],
        )
        .unwrap();
    }
    for i in 0..1000 {
        let value = json!({"text": "x".repeat(4096), "source": "agent", "at": now}).to_string();
        conn.execute(
            "INSERT INTO tenant_state_kv (tenant_id, key, value_json, updated_unix) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![tenant_id, format!("notes/{i:04}"), value, now + i],
        )
        .unwrap();
    }
    for i in 0..10 {
        conn.execute(
            "INSERT INTO runs (id, tenant_id, tool_name, trigger, status, started_unix) \
             VALUES (?1, ?2, ?3, 'job', 'running', ?4)",
            rusqlite::params![format!("01JRUN{i:04}"), tenant_id, format!("{}{i:02}", "t".repeat(39)), now],
        )
        .unwrap();
    }
}

#[tokio::test]
async fn a_tenant_at_the_free_plan_ceilings_resumes_in_4096_bytes_and_names_every_cut() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "RSM AC1").await;
    let releases = parse_changelog(CHANGELOG_MD).expect("changelog parses");
    let since = releases[20].version.to_string();
    seed_ceiling_tenant(&server, &ns, &since);

    let client = McpClient::with_bearer(&server.base_url, &key);
    let result = client.tools_call("host.resume", json!({})).await.expect("host.resume");
    let out = extract_structured(&result);
    let bytes = serde_json::to_vec(&out).unwrap().len();
    assert!(bytes <= BUDGET.max_bytes, "{bytes} bytes: {out}");

    assert_eq!(out["host"]["version_at_last_call"], since.as_str());
    let truncated = &out["truncated"];
    assert_eq!(truncated["tools"], true, "50 tools > the {} cap", BUDGET.tools);
    assert_eq!(truncated["notes"], true, "1000 notes > the {} cap", BUDGET.notes);
    assert_eq!(truncated["changes"], true, "20 releases > the {} cap", BUDGET.changes);
    // Whatever the byte budget shed, the section it shed from says so.
    for (section, shown) in [
        ("tools", out["tools"].as_array().unwrap().len()),
        ("notes", out["notes"].as_array().unwrap().len()),
        ("runs_open", out["runs_open"].as_array().unwrap().len()),
        ("changes", out["host"]["changes"].as_array().unwrap().len()),
    ] {
        let seeded = match section {
            "tools" => 50,
            "notes" => 1000,
            "runs_open" => 10,
            _ => 20,
        };
        if shown < seeded {
            assert_eq!(truncated[section], true, "{section}: showing {shown} of {seeded} without saying so");
        }
    }
    assert_eq!(out["next_call"]["tool"], "host.tool.test");
}

#[test]
fn the_budget_caps_are_the_prd_numbers() {
    let ResumeBudget { max_bytes, tools, notes, note_chars, runs, changes, .. } = BUDGET;
    assert_eq!((max_bytes, tools, notes, note_chars, runs, changes), (4096, 25, 10, 200, 10, 5));
}

/// The falsifier: every cap populated at once, at the largest sizes the store
/// allows, still renders within the byte budget.
#[test]
fn every_cap_at_its_worst_case_renders_within_the_byte_budget() {
    use mcphost::notes::Attach;
    use mcphost::resume::{LastCall, NoteEntry, RunEntry, Snapshot, ToolEntry, render};
    let snap = Snapshot {
        namespace: "n".repeat(64),
        last_seen: Some(1_760_000_000),
        version: "0.112.0".to_string(),
        version_at_last_call: Some("0.092.0".to_string()),
        changes: (0..BUDGET.changes * 4)
            .map(|i| json!({"version": format!("0.{}.0", 100 + i), "date": "2026-10-19", "summary": "s".repeat(300)}))
            .collect(),
        tools: (0..50)
            .map(|i| ToolEntry {
                name: format!("{}{i:02}", "t".repeat(39)),
                kind: "python".to_string(),
                last_call: Some(LastCall { at: 1_760_000_000 + i, ok: false, error_class: Some("tool_exception".to_string()) }),
            })
            .collect(),
        runs: (0..BUDGET.runs)
            .map(|i| RunEntry {
                id: format!("01J{}", "R".repeat(23)),
                tool: format!("{}{i:02}", "t".repeat(39)),
                status: "running".to_string(),
                started: Some(1_760_000_000),
            })
            .collect(),
        notes: (0..1000)
            .map(|i| NoteEntry {
                key: format!("notes/01J{}{i:03}", "N".repeat(20)),
                text: "x".repeat(4096),
                attached_to: Some(Attach::Tool("t".repeat(41))),
            })
            .collect(),
    };
    let out = render(&snap, &BUDGET);
    let bytes = serde_json::to_vec(&out).unwrap().len();
    assert!(bytes <= BUDGET.max_bytes, "{bytes} bytes");
    assert_eq!(out["truncated"]["tools"], true);
    assert_eq!(out["truncated"]["notes"], true);
    assert_eq!(out["truncated"]["changes"], true);
}

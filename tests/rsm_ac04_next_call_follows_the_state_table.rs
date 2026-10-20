//! contract: reads registry handler::host_tool_descriptors
//! PRD-mcphost-resume-tool AC4 -- Given each `ResumeState` in turn (no tools;
//! a tool never tested; a failed last call; an open run; all green), When
//! `host.resume` is called, Then `next_call.tool` is the row `RESUME_NEXT`
//! names for that state and that tool exists in `tools/list`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, publish, signup};
use mcphost::handler::{canonical_control_plane_names, host_tool_descriptors};
use mcphost::kinds::KindRegistry;
use mcphost::resume::{RESUME_NEXT, ResumeState, next_row};
use serde_json::{Value, json};

struct Fx {
    server: TestServer,
    client: McpClient,
    tenant_id: i64,
}

async fn fixture(label: &str) -> Fx {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, label).await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let conn = rusqlite::Connection::open(server.state.db.data_dir().join("mcphost.db")).unwrap();
    let tenant_id = conn.query_row("SELECT id FROM tenants WHERE namespace = ?1", [&ns], |r| r.get(0)).unwrap();
    Fx { server, client, tenant_id }
}

impl Fx {
    fn db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.server.state.db.data_dir().join("mcphost.db")).unwrap()
    }

    async fn publish_echo(&self, name: &str) {
        publish(&self.client, name, "echo", json!({"schema": {"type": "object"}})).await;
    }

    fn call_row(&self, tool: &str, ok: bool) {
        self.db()
            .execute(
                "INSERT INTO calls (tenant_id, tool_name, started_at, started_unix, duration_ms, ok, error_class) \
                 VALUES (?1, ?2, 'unix:1.0', 1760000000, 3, ?3, ?4)",
                rusqlite::params![self.tenant_id, tool, ok, (!ok).then_some("tool_exception")],
            )
            .unwrap();
    }

    async fn resume(&self) -> Value {
        extract_structured(&self.client.tools_call("host.resume", json!({})).await.expect("host.resume"))
    }

    async fn listed(&self) -> Vec<String> {
        let listed = self.client.tools_list().await.expect("tools/list");
        listed["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
    }
}

async fn assert_state(fx: &Fx, state: ResumeState, args: Value) {
    let out = fx.resume().await;
    let (_, tool, why) = *next_row(state);
    assert_eq!(out["next_call"]["tool"], tool, "{}: {out}", state.name());
    assert_eq!(out["next_call"]["why"], why, "{}", state.name());
    assert_eq!(out["next_call"]["args"], args, "{}", state.name());
    assert!(fx.listed().await.iter().any(|n| n == tool), "{tool} is not in tools/list");
}

#[tokio::test]
async fn no_tools_points_at_quickstart() {
    let fx = fixture("RSM AC4 none").await;
    assert_state(&fx, ResumeState::NoTools, json!({})).await;
}

#[tokio::test]
async fn a_tool_never_called_points_at_the_test_tool() {
    let fx = fixture("RSM AC4 untested").await;
    fx.publish_echo("fresh").await;
    assert_state(&fx, ResumeState::ToolUntested, json!({"name": "fresh"})).await;
}

#[tokio::test]
async fn a_failed_last_call_points_at_the_test_tool_for_that_tool() {
    let fx = fixture("RSM AC4 failed").await;
    fx.publish_echo("cleaner").await;
    fx.call_row("cleaner", false);
    assert_state(&fx, ResumeState::LastCallFailed, json!({"name": "cleaner"})).await;
}

#[tokio::test]
async fn an_open_run_points_at_runs_get() {
    let fx = fixture("RSM AC4 run").await;
    fx.publish_echo("worker").await;
    fx.call_row("worker", true);
    fx.db()
        .execute(
            "INSERT INTO runs (id, tenant_id, tool_name, trigger, status, started_unix) \
             VALUES ('01JRSMAC4RUN', ?1, 'worker', 'job', 'running', 1760000000)",
            [fx.tenant_id],
        )
        .unwrap();
    assert_state(&fx, ResumeState::RunOpen, json!({"run_id": "01JRSMAC4RUN"})).await;
    let out = fx.resume().await;
    assert_eq!(out["runs_open"][0]["id"], "01JRSMAC4RUN");

    // A finished run is not open.
    fx.db().execute("UPDATE runs SET status = 'done' WHERE id = '01JRSMAC4RUN'", []).unwrap();
    assert_state(&fx, ResumeState::AllGreen, json!({})).await;
}

#[tokio::test]
async fn all_green_points_at_the_tool_list() {
    let fx = fixture("RSM AC4 green").await;
    fx.publish_echo("steady").await;
    fx.call_row("steady", true);
    assert_state(&fx, ResumeState::AllGreen, json!({})).await;
}

/// Drift: every state has exactly one row, and every tool a row names is a
/// canonical `host.*` tool.
#[test]
fn every_state_has_one_row_and_every_named_tool_is_a_host_tool() {
    let kinds = KindRegistry::with_builtin();
    let canonical = canonical_control_plane_names(&kinds);
    let described: Vec<String> = host_tool_descriptors(&kinds).iter().map(|t| t.name.to_string()).collect();
    for state in ResumeState::ALL {
        assert_eq!(RESUME_NEXT.iter().filter(|r| r.0 == state).count(), 1, "{state:?}");
    }
    for (state, tool, _) in RESUME_NEXT {
        assert!(canonical.iter().any(|n| n == tool), "{state:?} names {tool}, which is not canonical");
        assert!(described.iter().any(|n| n == tool), "{state:?} names {tool}, which host_tools does not describe");
    }
}

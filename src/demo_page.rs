//! PRD-mcphost-demo-url-per-tool: `GET /d/{namespace}/{tool}` -- a public,
//! read-only page for a tool shared with `visibility = "url"` showing its
//! most recent runs. Like `/x/{token}/{tool}` (see [`crate::public_tool`])
//! it is public by construction; unlike it, it never runs anything.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::db::RunRow;
use crate::state::AppState;

/// Rows shown on the page (R1).
pub const DEMO_ROWS: i64 = 10;
/// Per-field excerpt cap, in characters (R1).
const EXCERPT_CHARS: usize = 200;

/// `<public_url>/d/<ns>/<tool>` -- same `trim_end_matches('/')` convention
/// as [`crate::public_tool::public_tool_url`].
pub fn demo_url(state: &AppState, namespace: &str, tool: &str) -> String {
    format!("{}/d/{}/{}", state.public_url.trim_end_matches('/'), namespace, tool)
}

/// R5: coarse caller class for the `demo_view` log line.
fn ua_class(headers: &HeaderMap) -> &'static str {
    let ua = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    const BOTS: &[&str] = &["bot", "crawl", "spider", "slurp", "preview", "facebookexternalhit"];
    const AGENTS: &[&str] = &[
        "curl", "wget", "python", "node", "axios", "go-http", "okhttp", "httpx", "reqwest", "claude",
        "anthropic", "openai", "gpt",
    ];
    if BOTS.iter().any(|b| ua.contains(b)) {
        "bot"
    } else if ua.is_empty() || AGENTS.iter().any(|a| ua.contains(a)) {
        "agent"
    } else {
        "browser"
    }
}

/// Truncates to [`EXCERPT_CHARS`] characters (never mid-codepoint).
fn excerpt(text: &str) -> String {
    text.chars().take(EXCERPT_CHARS).collect()
}

/// Unix seconds -> `YYYY-MM-DD HH:MM:SS` UTC (no chrono dependency).
fn utc_stamp(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// One run as a JSON row (shared by the HTML and `?format=json` views).
async fn run_row(state: &AppState, tenant_id: i64, run: &RunRow) -> Value {
    let args: Value = run
        .args_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(Value::Null);
    let result = if run.status == "done" {
        crate::runs::attach_result(state, tenant_id, run, json!({}))
            .await
            .ok()
            .and_then(|v| v.get("result").cloned())
            .unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    json!({
        "status": run.status,
        "duration_ms": run.duration_ms,
        "started_unix": run.started_unix,
        "args": excerpt(&args.to_string()),
        "result": excerpt(&result.to_string()),
    })
}

fn render_page(namespace: &str, tool: &str, rows: &[Value]) -> String {
    let mut body = String::new();
    for r in rows {
        body.push_str(&format!(
            "<tr class=\"run\"><td>{}</td><td>{}</td><td>{}</td><td><code>{}</code></td><td><code>{}</code></td></tr>",
            crate::claim::html_escape(r["status"].as_str().unwrap_or("")),
            r["duration_ms"].as_i64().map(|d| format!("{d} ms")).unwrap_or_default(),
            r["started_unix"].as_i64().map(utc_stamp).unwrap_or_default(),
            crate::claim::html_escape(r["args"].as_str().unwrap_or("")),
            crate::claim::html_escape(r["result"].as_str().unwrap_or("")),
        ));
    }
    include_str!("demo_page.html")
        .replace("__TITLE__", &crate::claim::html_escape(&format!("{namespace}.{tool}")))
        .replace("__ROWS__", &body)
}

/// `GET /d/{namespace}/{tool}` (R1, R5). Every miss -- unknown namespace,
/// unknown tool, tool not url-shared, disabled tenant -- is the same empty
/// 404, so the route leaks no existence information.
pub async fn demo_get(
    State(state): State<Arc<AppState>>,
    Path((namespace, tool)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let not_found = || StatusCode::NOT_FOUND.into_response();
    let Ok(Some(tenant)) = state.db.find_tenant_by_namespace(namespace.clone()).await else {
        return not_found();
    };
    if tenant.disabled {
        return not_found();
    }
    let Ok(Some(tool_row)) = state.db.get_tool(tenant.id, tool.clone()).await else {
        return not_found();
    };
    if tool_row.visibility != "url" {
        return not_found();
    }
    let Ok(runs) = state
        .db
        .list_runs(tenant.id, Some(tool.clone()), None, None, None, None, false, DEMO_ROWS)
        .await
    else {
        return not_found();
    };
    let mut rows = Vec::with_capacity(runs.len());
    for run in &runs {
        rows.push(run_row(&state, tenant.id, run).await);
    }
    let line = json!({"event": "demo_view", "tool": tool, "ua_class": ua_class(&headers)}).to_string();
    let _ = state.db.append_log(tenant.id, tool.clone(), line).await;
    crate::claim::html_response(StatusCode::OK, render_page(&namespace, &tool, &rows))
}

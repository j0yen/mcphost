//! Shared setup helpers for the "Uptime probes with no server" recipe
//! (PRD-mcphost-uptime-probes, `www/llms.txt`, `examples/uptime-probes/`):
//! the `targets`/`checks` tables, publishing `probe`/`status`, and
//! scheduling/firing `probe` the same way every AC file's own test does.
//!
//! Not a crate module: each `tests/*.rs` file is its own crate root, so
//! this is pulled in per-file with `#[path = "support/uptime_probes.rs"]
//! mod uptime_probes;`, same convention as `tests/support/host.rs`.

use crate::common::{ADMIN_KEY, McpClient, TestServer, extract_structured};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

// Not `#[path] mod egress_proxy_lock;` here: that would compile a second,
// separate `static LOCK` distinct from the one the
// `mcphost_sandbox_egress_allowlist_ac{03,06,07}_*.rs` files (which are
// always sandbox-classified into the same suite binary as this file's own
// callers) declare at the suite root -- defeating the shared lock. `crate::`
// reaches that single suite-root instance instead, same as this file's own
// `use crate::common::...` above.
use crate::egress_proxy_lock;

pub fn probe_source() -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/uptime-probes/tools/probe.py"),
    )
    .expect("read examples/uptime-probes/tools/probe.py")
}

pub fn status_source() -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/uptime-probes/tools/status.py"),
    )
    .expect("read examples/uptime-probes/tools/status.py")
}

/// Requirement 1: the `targets`/`checks` tables the recipe's own
/// `www/llms.txt` section shows.
pub async fn create_tables(client: &McpClient) {
    client
        .tools_call(
            "host.state.table_create",
            json!({
                "name": "targets",
                "schema": {"url": "text", "added_at": "real"},
                "primary_key": "url",
            }),
        )
        .await
        .expect("targets table_create");
    client
        .tools_call(
            "host.state.table_create",
            json!({
                "name": "checks",
                "schema": {"url": "text", "code": "integer", "ms": "real", "at": "real"},
            }),
        )
        .await
        .expect("checks table_create");
}

pub async fn publish_probe(client: &McpClient) {
    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "probe",
                "kind": "python",
                "spec": {"source": probe_source(), "network": "public"},
            }),
        )
        .await
        .expect("publish probe");
}

pub async fn publish_status(client: &McpClient) {
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "status", "kind": "python", "spec": {"source": status_source()}}),
        )
        .await
        .expect("publish status");
}

/// Sets a schedule trigger on `probe` (the recipe's own `*/5 * * * *`
/// schedule) and returns its `id`.
pub async fn set_probe_schedule(client: &McpClient) -> String {
    let set = extract_structured(
        &client
            .tools_call(
                "host.trigger.set",
                json!({"tool": "probe", "kind": "schedule", "schedule": "*/5 * * * *"}),
            )
            .await
            .expect("trigger.set"),
    );
    set["id"].as_str().expect("trigger id").to_string()
}

/// PRD-mcphost-sandbox-egress-allowlist AC10: these fixtures' own `probe`
/// tool has always declared `network: "public"` -- before this PRD that
/// worked for any tenant, Free included, since only the `"egress"` spelling
/// was plan-gated. Now `"public"`/`"egress"` are the same `pro`-only grant,
/// AND a `pro` tenant's egress needs `$MCPHOST_EGRESS_PROXY` configured to
/// get any sandbox network at all (AC3) -- so keeping these tests exercising
/// REAL network against their own local `wiremock`/mcphost servers (rather
/// than downgrading to `network: "none"` and losing that coverage) needs
/// both a `pro` tenant and a working proxy. Not a real deny-list proxy --
/// that enforcement is deploy-side (this PRD's own Non-goals) -- just a
/// bare HTTP/1.1 forward proxy, enough to let Python's `urllib.request`
/// (which honors `$MCPHOST_EGRESS_PROXY` automatically) reach the plain
/// `http://` targets these fixtures already stood up.
async fn proxy_one_connection(mut inbound: TcpStream) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = inbound.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 64 * 1024 {
            return Ok(());
        }
    }
    let line_end = buf.iter().position(|&b| b == b'\n').unwrap_or(buf.len());
    let request_line = String::from_utf8_lossy(&buf[..line_end]).trim_end().to_string();
    let mut parts = request_line.split(' ');
    let method = parts.next().unwrap_or("GET").to_string();
    let target = parts.next().unwrap_or("/").to_string();
    let version = parts.next().unwrap_or("HTTP/1.1").to_string();
    // Python's `urllib.request` proxying a plain `http://` request sends
    // the absolute-URI form on the request line (the standard HTTP forward-
    // proxy protocol) -- rewritten to origin-form (just the path) below so
    // the real origin server (wiremock/axum) routes it normally.
    let Some(rest) = target.strip_prefix("http://") else {
        return Ok(());
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (rest[..i].to_string(), rest[i..].to_string()),
        None => (rest.to_string(), "/".to_string()),
    };
    let host_port = if authority.contains(':') {
        authority
    } else {
        format!("{authority}:80")
    };
    let mut outbound = TcpStream::connect(&host_port).await?;
    outbound
        .write_all(format!("{method} {path} {version}\r\n").as_bytes())
        .await?;
    // Everything after the request line -- the rest of the headers, plus
    // any body bytes already read in the same initial read() -- forwards
    // verbatim; only the request line itself needed rewriting.
    outbound.write_all(&buf[line_end + 1..]).await?;
    tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await?;
    Ok(())
}

async fn start_forward_proxy() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind forward proxy");
    let addr = listener.local_addr().expect("forward proxy addr");
    tokio::spawn(async move {
        loop {
            let Ok((inbound, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let _ = proxy_one_connection(inbound).await;
            });
        }
    });
    format!("http://{addr}")
}

/// Starts the forward proxy above, points `$MCPHOST_EGRESS_PROXY` at it,
/// and promotes `ns` to `pro` via `admin.plan_set` -- everything a
/// `network: "public"` `probe` publish/call now needs (AC1/AC3).
///
/// Returns the held `egress_proxy_lock` guard: `cargo test`'s default
/// (no nextest) model runs every `#[tokio::test]` fn in a `tests/suite_*.rs`
/// binary concurrently as threads within ONE process, so this and every
/// `mcphost_sandbox_egress_allowlist_ac{03,06,07}_*.rs` test that also
/// mutates the process-wide `$MCPHOST_EGRESS_PROXY` share one lock -- the
/// caller must keep the returned guard alive for its whole test body (not
/// just this call), same as those AC files do themselves.
pub async fn grant_egress(server: &TestServer, ns: &str) -> tokio::sync::MutexGuard<'static, ()> {
    let guard = egress_proxy_lock::guard().await;
    let proxy = start_forward_proxy().await;
    // SAFETY: held across the caller's whole test body via the returned
    // guard above, so no other test in this binary observes a torn env var.
    unsafe {
        std::env::set_var("MCPHOST_EGRESS_PROXY", &proxy);
    }
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    admin
        .tools_call(
            "admin.plan_set",
            json!({"tenant": ns, "plan": "pro", "reason": "uptime-probes fixture needs egress (PRD-mcphost-sandbox-egress-allowlist)"}),
        )
        .await
        .expect("admin.plan_set to pro");
    guard
}

/// Fires the trigger once and waits for the resulting run to leave
/// `queued`/`running`, same convention as
/// `tests/sched_ac10_trigger_fire_manual.rs`'s own poll loop.
pub async fn fire_and_wait(client: &McpClient, trigger_id: &str) -> Value {
    let fired = extract_structured(
        &client
            .tools_call("host.trigger.fire", json!({"id": trigger_id}))
            .await
            .expect("trigger.fire"),
    );
    let run_id = fired["run_id"].as_str().expect("run_id").to_string();

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let got = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": run_id}))
                .await
                .expect("runs.get"),
        );
        if !matches!(got["status"].as_str(), Some("queued") | Some("running")) {
            return got;
        }
        if Instant::now() >= deadline {
            panic!("probe run never left queued/running within 20s: {got:?}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

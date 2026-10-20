//! PRD-mcphost-email-send-live-proof
//! AC5 (P0) -- Given `send --verify dns` with `MCPHOST_PROBE_DNS_ZONE`,
//! `MCPHOST_PROBE_CF_TOKEN` and `MCPHOST_PROBE_CF_API` pointing at a stub that
//! stores the published records, and the server's resolver injected to answer
//! from that stub, When it runs, Then it publishes the TXT record
//! `host.claim.dns.start` returned, reaches `owner_verified: true` through
//! `host.claim.dns.check`, deletes the record on exit and the send proceeds to
//! `status=sent`; Given neither the DNS env nor `MCPHOST_ADMIN_KEY`, When
//! `send` runs with `--verify auto` (or an explicit `--verify dns`), Then it
//! prints `status=error reason=verify_path_unavailable` and exits 2 before any
//! `host.email.send` call.

use crate::common;
use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use common::TestServer;
use mcphost::email::FakeEmailClient;
use mcphost::verify::{DnsResolver, OwnerVerify};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const ZONE: &str = "probe.example.dev";
const CF_TOKEN: &str = "cf-stub-token";

/// The Cloudflare stand-in's state: `(record id, name, content)` rows, plus every call it saw.
#[derive(Default)]
struct Cloudflare {
    records: Mutex<Vec<(String, String, String)>>,
    calls: Mutex<Vec<String>>,
}

/// The injected resolver answers from the stub's stored records.
struct StubResolver(Arc<Cloudflare>);

#[async_trait::async_trait]
impl DnsResolver for StubResolver {
    async fn txt(&self, name: &str) -> Result<Vec<String>, String> {
        Ok(self.0.records.lock().unwrap().iter().filter(|(_, n, _)| n == name).map(|(_, _, c)| c.clone()).collect())
    }
}

async fn start_cloudflare(cf: Arc<Cloudflare>) -> String {
    async fn zones(State(cf): State<Arc<Cloudflare>>, Query(q): Query<HashMap<String, String>>) -> Json<Value> {
        cf.calls.lock().unwrap().push(format!("GET zones name={}", q.get("name").cloned().unwrap_or_default()));
        Json(json!({"success": true, "result": [{"id": "zone1"}]}))
    }
    async fn create(State(cf): State<Arc<Cloudflare>>, Path(zone): Path<String>, Json(body): Json<Value>) -> Json<Value> {
        let mut records = cf.records.lock().unwrap();
        let id = format!("rec{}", records.len() + 1);
        cf.calls.lock().unwrap().push(format!("POST {zone} type={}", body["type"].as_str().unwrap_or_default()));
        records.push((id.clone(), body["name"].as_str().unwrap().to_string(), body["content"].as_str().unwrap().to_string()));
        Json(json!({"success": true, "result": {"id": id}}))
    }
    async fn remove(State(cf): State<Arc<Cloudflare>>, Path((zone, id)): Path<(String, String)>) -> Json<Value> {
        cf.calls.lock().unwrap().push(format!("DELETE {zone} {id}"));
        cf.records.lock().unwrap().retain(|(rid, _, _)| *rid != id);
        Json(json!({"success": true, "result": {"id": id}}))
    }
    let app = Router::new()
        .route("/zones", get(zones))
        .route("/zones/{zone}/dns_records", post(create))
        .route("/zones/{zone}/dns_records/{id}", delete(remove))
        .with_state(cf);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

async fn run_probe(args: &[&str], env: Vec<(&'static str, String)>) -> (String, i32) {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    tokio::task::spawn_blocking(move || {
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/email-probe.sh");
        let mut cmd = std::process::Command::new("bash");
        cmd.arg(script).args(args).env_clear().env("PATH", std::env::var("PATH").unwrap_or_default());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let out = cmd.output().expect("run script");
        (
            format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)),
            out.status.code().unwrap_or(-1),
        )
    })
    .await
    .expect("join")
}

#[tokio::test]
async fn emslp_ac05_dns_path_publishes_verifies_deletes_and_sends() {
    let cf = Arc::new(Cloudflare::default());
    let cf_url = start_cloudflare(cf.clone()).await;
    let fake = Arc::new(FakeEmailClient::new());
    let ov = OwnerVerify { github: None, resolver: Arc::new(StubResolver(cf.clone())) };
    let server = TestServer::start_with_owner_verify_and_email(ov, fake.clone()).await;

    let (out, rc) = run_probe(
        &["send", "--verify", "dns"],
        vec![
            ("MCPHOST_URL", server.base_url.clone()),
            ("MCPHOST_PROBE_DNS_ZONE", ZONE.to_string()),
            ("MCPHOST_PROBE_CF_TOKEN", CF_TOKEN.to_string()),
            ("MCPHOST_PROBE_CF_API", cf_url),
            ("MCPHOST_PROBE_DNS_POLL_SECS", "1".to_string()),
        ],
    )
    .await;
    assert_eq!(rc, 0, "{out}");
    let lines: Vec<&str> = out.lines().collect();
    let ns = lines[0].trim_start_matches("tenant=");
    assert_eq!(lines[1], "verify=dns", "{out}");
    assert!(lines[2].starts_with("status=sent id=em_"), "{out}");
    assert_eq!(lines.last().copied(), Some("offboarded=true"), "{out}");

    let calls = cf.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 3, "{calls:?}");
    assert_eq!(calls[0], format!("GET zones name={ZONE}"));
    assert_eq!(calls[1], "POST zone1 type=TXT");
    assert_eq!(calls[2], "DELETE zone1 rec1");
    assert!(cf.records.lock().unwrap().is_empty(), "the record is deleted on exit");

    let row = server.state.db.find_tenant_by_namespace(ns.to_string()).await.unwrap().unwrap();
    assert_eq!(server.state.db.owner_provenance(row.id).await.unwrap().0.as_deref(), Some("dns"));
    let slug = ns.to_ascii_lowercase().replace('_', "-");
    assert_eq!(server.state.db.owner_provenance(row.id).await.unwrap().1, Some(format!("{slug}.{ZONE}")));
    assert_eq!(fake.send_count(), 1);
}

#[tokio::test]
async fn emslp_ac05_no_verify_path_exits_2_before_any_send() {
    let fake = Arc::new(FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    for args in [vec!["send"], vec!["send", "--verify", "dns"]] {
        let (out, rc) = run_probe(&args, vec![("MCPHOST_URL", server.base_url.clone())]).await;
        assert_eq!(rc, 2, "{args:?}: {out}");
        assert!(out.contains("status=error reason=verify_path_unavailable"), "{out}");
        assert!(out.contains("offboarded=true"), "{out}");
    }
    assert_eq!(fake.send_count(), 0);
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open raw db");
    let rows: i64 = conn.query_row("SELECT COUNT(*) FROM email_sends", [], |r| r.get(0)).expect("count");
    assert_eq!(rows, 0, "no host.email.send call was made, not even a refused one");
}

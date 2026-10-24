//! PRD-mcphost-external-unclaimed-tenant-expiry: fixture helpers for the `texp_ac*` suites.

use rusqlite::{Connection, params};

pub const POLICY: &str = "external_unclaimed_tenants";
pub const ENV: &str = "MCPHOST_RETENTION_EXTERNAL_UNCLAIMED_TENANT_IDLE_DAYS";
/// The sibling policy row that owns `floor_unix`.
pub const FLOOR_POLICY: &str = "synthetic_implicit_tenants";
pub const DAY: i64 = 86_400;

/// Moves the shared floor (test-only: the real one is the first seed time).
pub fn set_floor(conn: &Connection, unix: i64) {
    conn.execute("UPDATE retention_policy SET floor_unix = ?1 WHERE table_name = ?2", params![unix, FLOOR_POLICY])
        .expect("set floor");
}

/// Shapes an already-signed-up tenant into an `EXT` subject: external origin,
/// no claim or billing marker, minted `age_days` ago and last seen
/// `idle_days` ago (`None` = never seen).
pub fn make_external(conn: &Connection, ns: &str, now: i64, age_days: i64, idle_days: Option<i64>) -> i64 {
    conn.execute(
        "UPDATE tenants SET origin = 'external', signup_source = 'implicit', synthetic = NULL, \
         claimed_unix = NULL, owner_verified_at = NULL, owner_email = NULL, paid_unix = NULL, \
         billing_ref = NULL, stripe_customer_id = NULL, plan = 'free', created_unix = ?2, \
         last_seen_unix = ?3 WHERE namespace = ?1",
        params![ns, now - age_days * DAY, idle_days.map(|d| now - d * DAY)],
    )
    .expect("shape the tenant");
    id_of(conn, ns)
}

pub fn id_of(conn: &Connection, ns: &str) -> i64 {
    conn.query_row("SELECT id FROM tenants WHERE namespace = ?1", params![ns], |r| r.get(0)).expect("tenant id")
}

/// `(disabled, disabled_reason)` of one tenant row.
pub fn state(conn: &Connection, id: i64) -> (bool, Option<String>) {
    conn.query_row("SELECT disabled, disabled_reason FROM tenants WHERE id = ?1", params![id], |r| {
        Ok((r.get::<_, i64>(0)? != 0, r.get(1)?))
    })
    .expect("tenant row")
}

/// Stage names of one tenant's lifecycle events, oldest first.
pub fn stages(conn: &Connection, id: i64) -> Vec<String> {
    let mut stmt = conn
        .prepare("SELECT stage FROM tenant_lifecycle_events WHERE tenant_id = ?1 ORDER BY id")
        .expect("prepare");
    stmt.query_map(params![id], |r| r.get(0)).expect("query").collect::<Result<_, _>>().expect("rows")
}

/// Count of one tenant's events at `stage`.
pub fn count_stage(conn: &Connection, id: i64, stage: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM tenant_lifecycle_events WHERE tenant_id = ?1 AND stage = ?2",
        params![id, stage],
        |r| r.get(0),
    )
    .expect("count")
}

/// Writes one lifecycle event `created_unix` (a back-dated stage).
pub fn add_event(conn: &Connection, id: i64, stage: &str, idle_since: i64, created_unix: i64) {
    conn.execute(
        "INSERT INTO tenant_lifecycle_events (tenant_id, policy, stage, idle_since_unix, created_unix) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, POLICY, stage, idle_since, created_unix],
    )
    .expect("insert event");
}

/// A claim attempt: one `claim_codes` row for `email`.
pub fn add_claim_attempt(conn: &Connection, id: i64, email: &str, created_unix: i64) {
    conn.execute(
        "INSERT INTO claim_codes (tenant_id, code_hash, email, expires_unix, created_unix) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, format!("texp-{id}-{email}-{created_unix}"), email, created_unix + 1800, created_unix],
    )
    .expect("insert claim attempt");
}

/// `(last_seen_unix, created_unix)` of one tenant row.
pub fn clock(conn: &Connection, id: i64) -> (Option<i64>, i64) {
    conn.query_row("SELECT last_seen_unix, created_unix FROM tenants WHERE id = ?1", params![id], |r| {
        Ok((r.get(0)?, r.get(1)?))
    })
    .expect("clock")
}

use super::{ADMIN_KEY, McpClient, TestServer, extract_structured};
use async_trait::async_trait;
use mcphost::email::FakeEmailClient;
use mcphost::kinds::{Kind, KindError, KindRegistry, ToolDescriptor};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

/// A server with a configured, inspectable fake email client, the given
/// kinds and a signup limit high enough to mint several implicit tenants.
pub async fn server(email: Arc<FakeEmailClient>, kinds: KindRegistry) -> TestServer {
    TestServer::start_full_with_email(
        Some(ADMIN_KEY.to_string()),
        kinds,
        mcphost::state::CALL_TIMEOUT,
        None,
        1000,
        mcphost::billing::BillingConfig::default(),
        Arc::new(mcphost::billing::FakeBillingClient::new(mcphost::state::now_unix())),
        Vec::new(),
        mcphost::email::EmailConfig {
            api_url: Some("https://email.invalid/send".to_string()),
            api_key: Some("test-email-key".to_string()),
            from: Some("noreply@mcphost.invalid".to_string()),
            provider: mcphost::email::EmailProvider::default(),
        },
        email,
        mcphost::state::CLAIM_TOKEN_TTL_SECS_DEFAULT,
        mcphost::state::CLAIM_RATE_LIMIT_PER_HOUR_DEFAULT,
        mcphost::db::DbConfig::from_env(),
        mcphost::alerts::AlertConfig::default(),
        mcphost::state::FleetIps::empty(),
        mcphost::state::VerifiedClientIds::empty(),
        Vec::new(),
    )
    .await
}

/// Records every tenant id `on_tenant_removed` is called with.
pub struct HookKind(pub Arc<Mutex<Vec<i64>>>);

#[async_trait]
impl Kind for HookKind {
    fn contract(&self) -> &'static mcphost::kinds::contract::KindContract {
        &mcphost::kinds::contract::EMPTY_CONTRACT
    }
    fn name(&self) -> &'static str {
        "texphook"
    }
    fn validate(&self, _spec: &Value) -> Result<(), KindError> {
        Ok(())
    }
    fn describe(&self, _spec: &Value) -> ToolDescriptor {
        ToolDescriptor {
            name: "texphook".to_string(),
            description: "records tenant removal".to_string(),
            input_schema: json!({"type": "object"}),
        }
    }
    async fn call(&self, _spec: &Value, _args: Value, _ctx: &mcphost::kinds::CallCtx) -> Result<Value, KindError> {
        Ok(json!({}))
    }
    async fn on_tenant_removed(&self, tenant_id: i64) {
        self.0.lock().unwrap().push(tenant_id);
    }
}

/// An implicit-signup tenant with a key and a `/u/{secret}/mcp` path.
pub struct Minted {
    pub ns: String,
    pub key: String,
    pub path: String,
}

/// Keyless mint, then one rotation to learn the key and current URL.
pub async fn mint(server: &TestServer) -> Minted {
    let first = extract_structured(
        &McpClient::new(&server.base_url).tools_call("host.whoami", json!({})).await.expect("implicit mint"),
    );
    let ns = first["tenant"].as_str().expect("tenant").to_string();
    let url = first["onboarding"]["url"].as_str().expect("onboarding.url").to_string();
    let by_url = McpClient::new(&server.base_url).with_path(url.trim_start_matches(&server.base_url));
    let rotated = extract_structured(&by_url.tools_call("host.key.rotate", json!({})).await.expect("rotate"));
    Minted {
        ns,
        key: rotated["key"].as_str().expect("key").to_string(),
        path: rotated["url"].as_str().expect("url").trim_start_matches(&server.base_url).to_string(),
    }
}

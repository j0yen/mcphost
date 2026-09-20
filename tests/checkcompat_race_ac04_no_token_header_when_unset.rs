//! PRD-mcphost-checkcompat-port-race AC4 (P0) — Given
//! `MCPHOST_COMPAT_TOKEN` unset, When `/healthz` is requested, Then no
//! `X-Mcphost-Compat-Token` header is present.
//!
//! `tests/common::TestServer` never sets `AppState::compat_token` (it
//! builds `AppState` directly, field `None`) -- exactly "unset" from
//! `/healthz`'s point of view, since requirement 5 gates the header on
//! that field, not on the process's real env. Checked on both the
//! anonymous and admin response shapes (`src/http.rs::healthz` early-
//! returns for the anonymous case), so a future refactor can't
//! reintroduce the header on only one branch.

use crate::common;
use common::TestServer;

#[tokio::test]
async fn healthz_carries_no_compat_token_header_when_unset() {
    let server = TestServer::start().await;
    let client = reqwest::Client::new();

    // Anonymous request (no bearer): the early-return branch.
    let resp = client
        .get(format!("{}/healthz", server.base_url))
        .send()
        .await
        .expect("GET /healthz (anonymous)");
    assert!(
        resp.headers().get("X-Mcphost-Compat-Token").is_none(),
        "anonymous /healthz must not carry X-Mcphost-Compat-Token when MCPHOST_COMPAT_TOKEN is unset"
    );

    // Admin request: the full-document branch.
    let resp = client
        .get(format!("{}/healthz", server.base_url))
        .bearer_auth(common::ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz (admin)");
    assert!(
        resp.headers().get("X-Mcphost-Compat-Token").is_none(),
        "admin /healthz must not carry X-Mcphost-Compat-Token when MCPHOST_COMPAT_TOKEN is unset"
    );
}

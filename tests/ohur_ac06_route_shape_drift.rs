//! AC6 — Given the `hooks.rs` route-shape constants, When a test builds a
//! URL with `event_url` and classifies it, Then the class is
//! `own_hook_event`; and the axum mount cannot drift from the constants.

use crate::common;
use mcphost::hooks::{EVENT_ROUTE, INBOX_ROUTE, OwnHookPath, classify_own_hook_path, event_url};
use mcphost::http::routes::ROUTES;

fn mounted(path: &str) -> bool {
    ROUTES.iter().any(|r| r.method == "POST" && r.path == path)
}

#[tokio::test]
async fn ohur_ac06() {
    let server = common::TestServer::start().await;
    let url = event_url(&server.state, "t_a", "push_reactor_1");
    let path = url.strip_prefix(server.state.public_url.trim_end_matches('/')).expect("url under public_url");
    let class = classify_own_hook_path(path).expect("event_url output classifies");
    assert_eq!(class, OwnHookPath::Event { namespace: "t_a".into(), tool: "push_reactor_1".into() });
    assert_eq!(class.remedy_code(), "host_not_allowed.own_hook_event");

    let webhook = mcphost::webhooks::webhook_url(&server.state, "01ABC");
    let path = webhook.strip_prefix(server.state.public_url.trim_end_matches('/')).expect("under public_url");
    assert_eq!(classify_own_hook_path(path), Some(OwnHookPath::Inbox { id: "01ABC".into() }));

    // Drift: the constants ARE the mounted paths...
    assert!(mounted(EVENT_ROUTE), "{EVENT_ROUTE} is not mounted in http/routes.rs");
    assert!(mounted(INBOX_ROUTE), "{INBOX_ROUTE} is not mounted in http/routes.rs");
    // ...and a copy of the constant with a changed path is caught.
    assert!(!mounted(&EVENT_ROUTE.replace("/hooks/", "/hooks2/")));
    assert_eq!(classify_own_hook_path("/hooks2/t_a/x"), None);
    assert_eq!(classify_own_hook_path("/api/x"), None);
    assert_eq!(classify_own_hook_path("/hooks/t_a"), None);
}

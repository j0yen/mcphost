//! AC1 — Given a keyed tenant on `build_router` with the prod transport, When
//! it POSTs `subscriptions/listen` for tools, prompts and resources list
//! changes, Then it gets 200 `text/event-stream` with `X-Accel-Buffering: no`,
//! first frame `notifications/subscriptions/acknowledged` (subscription id =
//! request id, filter exactly tools and prompts), then at least one keep-alive
//! comment within 16 s and no other frame.

use crate::common;
use common::{ListenFrame, ListenStream, TestServer};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn sublis_ac01_listen_acks_then_keeps_alive() {
    let server = TestServer::start().await;
    let (_tenant, key) = common::signup(&server.base_url, "sublis1").await;

    let mut listen = ListenStream::open_with(
        &server.base_url,
        Some(&key),
        4242,
        json!({"toolsListChanged": true, "promptsListChanged": true, "resourcesListChanged": true}),
    )
    .await;

    assert_eq!(listen.status, 200, "a keyed listen is accepted, not 404");
    let content_type = listen.headers.get("content-type").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
    assert!(content_type.starts_with("text/event-stream"), "listen streams as SSE, got {content_type:?}");
    assert_eq!(
        listen.headers.get("x-accel-buffering").and_then(|v| v.to_str().ok()),
        Some("no"),
        "the stream tells the proxy not to buffer it"
    );

    let first = listen.next_frame(Duration::from_secs(5)).await.expect("an acknowledgement frame");
    let ListenFrame::Message(ack) = first else { panic!("first frame must be the acknowledgement, got {first:?}") };
    assert_eq!(ack["method"], "notifications/subscriptions/acknowledged");
    assert_eq!(ack["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"], 4242, "subscription id is the request id");
    assert_eq!(
        ack["params"]["notifications"],
        json!({"toolsListChanged": true, "promptsListChanged": true}),
        "filter is exactly tools and prompts"
    );

    let second = listen.next_frame(Duration::from_secs(16)).await.expect("a keep-alive within 16 s");
    assert!(matches!(second, ListenFrame::Comment(_)), "the only thing after the ack is a keep-alive comment, got {second:?}");
}

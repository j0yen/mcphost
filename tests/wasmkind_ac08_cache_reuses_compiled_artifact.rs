//! PRD-mcphost-wasm-kind
//! AC8 -- Given a tool called twice, When the second call runs, Then it
//! reuses the cached compiled artifact (observable via a compile counter);
//! a republish invalidates the cache and the next call recompiles.

use crate::common;
use common::{signup, wasm_fixture_b64, wasm_kind_registry_with_handle};
use serde_json::json;

#[tokio::test]
async fn second_call_reuses_the_cache_and_republish_invalidates_it() {
    let (kinds, wasm) = wasm_kind_registry_with_handle();
    let server = common::TestServer::start_with_kinds(kinds).await;
    let (ns, key) = signup(&server.base_url, "Wasm AC8 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "echoer",
                "kind": "wasm",
                "spec": {"component": wasm_fixture_b64("echo")},
            }),
        )
        .await
        .expect("publish wasm echo tool");
    assert_eq!(
        wasm.compile_count(),
        0,
        "publish only validates (Component::new against a throwaway parse); it must not \
         populate the call-path cache or count as a compile"
    );

    client
        .tools_call(&format!("{ns}.echoer"), json!({"msg": "one"}))
        .await
        .expect("first call");
    assert_eq!(wasm.compile_count(), 1, "the first call compiles once");
    assert_eq!(wasm.cache_hit_count(), 0);

    client
        .tools_call(&format!("{ns}.echoer"), json!({"msg": "two"}))
        .await
        .expect("second call");
    assert_eq!(
        wasm.compile_count(),
        1,
        "a second call on the same tool must reuse the cached compiled artifact, not recompile"
    );
    assert_eq!(
        wasm.cache_hit_count(),
        1,
        "the second call must be observable as a cache hit"
    );

    // Republish (same name, same bytes) invalidates the cache entry outright.
    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "echoer",
                "kind": "wasm",
                "spec": {"component": wasm_fixture_b64("echo")},
            }),
        )
        .await
        .expect("republish wasm echo tool");

    client
        .tools_call(&format!("{ns}.echoer"), json!({"msg": "three"}))
        .await
        .expect("call after republish");
    assert_eq!(
        wasm.compile_count(),
        2,
        "a republish must invalidate the cache so the next call recompiles"
    );
}

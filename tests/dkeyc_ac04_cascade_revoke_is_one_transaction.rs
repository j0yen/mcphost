//! PRD-mcphost-key-delegate-child-keys
//! AC4 (P0) — Given `c1` → `c2` → `c3` live, When the primary calls
//! `host.key.revoke {label: "c1", cascade: true}`, Then `revoked == 3`, each
//! key's next call fails `key_revoked` with `data.revoked_by == "c1"`, and
//! the change is one transaction (a concurrent `c3` call sees either all live
//! or all revoked).

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

async fn delegate(base: &str, client: &McpClient, label: &str, ttl_s: i64) -> McpClient {
    let out: Value = extract_structured(
        &client
            .tools_call(
                "host.key.delegate",
                json!({"scopes": ["tools:call"], "ttl_s": ttl_s, "label": label, "can_delegate": true}),
            )
            .await
            .unwrap_or_else(|e| panic!("delegate {label}: {e:?}")),
    );
    McpClient::with_bearer(base, out["key"].as_str().expect("key"))
}

#[tokio::test]
async fn dkeyc_ac04_cascade_revokes_the_subtree_in_one_transaction() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Delegate Cascade Tenant").await;
    let primary = McpClient::with_bearer(&server.base_url, &key);
    let c1 = delegate(&server.base_url, &primary, "c1", 3000).await;
    let c2 = delegate(&server.base_url, &c1, "c2", 2000).await;
    let c3 = delegate(&server.base_url, &c2, "c3", 1000).await;
    for c in [&c1, &c2, &c3] {
        c.tools_call("host.whoami", json!({})).await.expect("live before revoke");
    }

    // Observer: while the revoke runs, the three rows are read together on
    // a raw connection and must always agree (all live or all revoked).
    let db_path = server.data_dir.0.join("mcphost.db");
    let observer = tokio::task::spawn_blocking(move || {
        let conn = rusqlite::Connection::open(db_path).expect("raw db");
        let mut seen_mixed = false;
        let mut saw_revoked = false;
        for _ in 0..400 {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM tenant_keys WHERE label IN ('c1','c2','c3') AND revoked_at IS NOT NULL",
                    [],
                    |r| r.get(0),
                )
                .expect("count revoked");
            seen_mixed |= n != 0 && n != 3;
            saw_revoked |= n == 3;
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        (seen_mixed, saw_revoked)
    });
    let concurrent_c3 = {
        let c3 = &c3;
        async move {
            let mut codes = Vec::new();
            for _ in 0..20 {
                codes.push(match c3.tools_call("host.whoami", json!({})).await {
                    Ok(_) => None,
                    Err(e) => e.error_code,
                });
            }
            codes
        }
    };
    let revoke = async {
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        extract_structured(
            &primary
                .tools_call("host.key.revoke", json!({"label": "c1", "cascade": true}))
                .await
                .expect("cascade revoke"),
        )
    };
    let (revoked, codes, observed) = tokio::join!(revoke, concurrent_c3, observer);
    assert_eq!(revoked["revoked"], json!(3), "{revoked}");
    assert_eq!(revoked["labels"], json!(["c1", "c2", "c3"]), "{revoked}");
    let (seen_mixed, _) = observed.expect("observer");
    assert!(!seen_mixed, "a raw reader saw a partly revoked tree");
    // The concurrent c3 caller sees live calls, then revoked ones -- never
    // live again after the first refusal.
    let first_refusal = codes.iter().position(|c| c.is_some());
    if let Some(i) = first_refusal {
        assert!(codes[i..].iter().all(|c| c.as_deref() == Some("key_revoked")), "{codes:?}");
    }

    for (name, c) in [("c1", &c1), ("c2", &c2), ("c3", &c3)] {
        let err = c.tools_call("host.whoami", json!({})).await.expect_err("revoked");
        assert_eq!(err.error_code.as_deref(), Some("key_revoked"), "{name}: {err:?}");
        assert_eq!(err.data["revoked_by"], json!("c1"), "{name}: {err:?}");
    }
}

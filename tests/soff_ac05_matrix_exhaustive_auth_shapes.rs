//! AC5 — `soff_matrix`: the generated auth-shape enum drives the matrix so a
//! new `AuthShape` variant causes a compile error here, naming the uncovered
//! shape.  The matrix also asserts R1 (success envelope on first call) and R2
//! (`already_offboarded` on second call) for every shape including the URL
//! secret path.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::auth_shape::AuthShape;
use serde_json::json;

#[tokio::test]
async fn soff_matrix() {
    let server = TestServer::start().await;

    for &shape in AuthShape::ALL {
        let (_ns, key) = signup(&server.base_url, &format!("Matrix {}", shape.name())).await;

        // For the URL shape, mint the path secret before the first call so
        // we can reuse it on the second call.
        let url_path: Option<String> = match shape {
            AuthShape::Url => {
                let setup = McpClient::with_bearer(&server.base_url, &key)
                    .with_session_continuity();
                let rotated = setup
                    .tools_call("host.key_rotate", json!({}))
                    .await
                    .expect("key_rotate for url shape");
                let url = extract_structured(&rotated)["url"]
                    .as_str()
                    .expect("url")
                    .to_string();
                Some(url.trim_start_matches(&server.base_url).to_string())
            }
            // Exhaustive: no `_ =>` arm.  Adding a new AuthShape variant
            // without a Url-equivalent setup block is a compile error.
            AuthShape::Header | AuthShape::Arg | AuthShape::Both => None,
        };

        // First call: must return the success envelope (R1).
        // Exhaustive match: no `_ =>` arm — compile error on new variant.
        let first = match shape {
            AuthShape::Header => {
                McpClient::with_bearer(&server.base_url, &key)
                    .tools_call("host.self.offboard", json!({}))
                    .await
            }
            AuthShape::Arg => {
                McpClient::new(&server.base_url)
                    .tools_call("host.self.offboard", json!({"tenant_key": key}))
                    .await
            }
            AuthShape::Both => {
                McpClient::with_bearer(&server.base_url, &key)
                    .tools_call("host.self.offboard", json!({"tenant_key": key}))
                    .await
            }
            AuthShape::Url => {
                let path = url_path.as_deref().expect("url_path set above");
                McpClient::new(&server.base_url)
                    .with_path(path)
                    .tools_call("host.self.offboard", json!({}))
                    .await
            }
        }
        .unwrap_or_else(|e| {
            panic!(
                "soff_matrix shape={}: first call must be ok (success envelope), got {e:?}",
                shape.name()
            )
        });
        let first = extract_structured(&first);
        assert_eq!(
            first["disabled"],
            json!(true),
            "shape={} first call: {first}",
            shape.name()
        );
        assert_eq!(
            first["disabled_reason"],
            json!("self_offboard"),
            "shape={} first call: {first}",
            shape.name()
        );
        assert!(
            first.get("already_offboarded").map_or(true, |v| v == &json!(false)),
            "shape={} first call must not have already_offboarded=true: {first}",
            shape.name()
        );

        // Second call: must return the idempotent already_offboarded envelope (R2).
        // Exhaustive match again — same compile-time contract.
        let second = match shape {
            AuthShape::Header => {
                McpClient::with_bearer(&server.base_url, &key)
                    .tools_call("host.self.offboard", json!({}))
                    .await
            }
            AuthShape::Arg => {
                McpClient::new(&server.base_url)
                    .tools_call("host.self.offboard", json!({"tenant_key": key}))
                    .await
            }
            AuthShape::Both => {
                McpClient::with_bearer(&server.base_url, &key)
                    .tools_call("host.self.offboard", json!({"tenant_key": key}))
                    .await
            }
            AuthShape::Url => {
                let path = url_path.as_deref().expect("url_path set above");
                McpClient::new(&server.base_url)
                    .with_path(path)
                    .tools_call("host.self.offboard", json!({}))
                    .await
            }
        }
        .unwrap_or_else(|e| {
            panic!(
                "soff_matrix shape={}: second call must be the already_offboarded envelope, not an error, got {e:?}",
                shape.name()
            )
        });
        let second = extract_structured(&second);
        assert_eq!(
            second["disabled"],
            json!(true),
            "shape={} second call: {second}",
            shape.name()
        );
        assert_eq!(
            second["already_offboarded"],
            json!(true),
            "shape={} second call must have already_offboarded: {second}",
            shape.name()
        );
    }
}

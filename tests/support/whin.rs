//! Shared fixture for the `whin_ac*` tests (PRD-mcphost-webhook-inbox-one-call):
//! a python-capable server that advertises `https://` hook URLs, one
//! signed-up tenant, and a signer/poster for the hook endpoint.

#![allow(dead_code)]

use crate::common::{McpClient, TempDataDir, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use serde_json::{Value, json};
use std::time::Duration;

pub struct Fixture {
    pub server: TestServer,
    pub client: McpClient,
    pub ns: String,
    pub key: String,
    _envs: TempDataDir,
}

pub async fn start() -> Fixture {
    let envs = TempDataDir::new();
    let server = TestServer::start_with_kinds_https_public(python_kind_registry(&envs.0)).await;
    let (ns, key) = signup(&server.base_url, "whin tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    Fixture { server, client, ns, key, _envs: envs }
}

pub fn hmac_sha256_hex(secret: &str, body: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut key_block = [0u8; 64];
    let key = secret.as_bytes();
    if key.len() > 64 {
        key_block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(key_block.map(|b| b ^ 0x36));
    inner.update(body);
    let inner_digest = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(key_block.map(|b| b ^ 0x5c));
    outer.update(inner_digest);
    outer.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

impl Fixture {
    pub async fn create(&self, args: Value) -> Result<Value, crate::common::RpcError> {
        self.client.tools_call("host.webhook_inbox.create", args).await.map(|v| extract_structured(&v))
    }

    pub async fn create_inbox(&self) -> Value {
        self.create(json!({"name": "inbox"})).await.unwrap_or_else(|e| panic!("create: {} {}", e.code, e.message))
    }

    /// The advertised `https://` URL pointed back at this server's plain
    /// HTTP listener.
    pub fn local_url(&self, advertised: &str) -> String {
        let path = advertised.split_once("/hook/").map(|(_, id)| id).expect("/hook/ url");
        format!("{}/hook/{path}", self.server.base_url)
    }

    pub async fn post(&self, url: &str, body: &[u8], signature: Option<String>) -> reqwest::Response {
        let mut req = reqwest::Client::new().post(self.local_url(url)).body(body.to_vec());
        if let Some(sig) = signature {
            req = req.header("X-Mcphost-Signature", format!("sha256={sig}"));
        }
        req.send().await.expect("post hook")
    }

    pub async fn post_signed(&self, url: &str, secret: &str, body: &[u8]) -> reqwest::Response {
        self.post(url, body, Some(hmac_sha256_hex(secret, body))).await
    }

    /// `host.tool_call(name)` once the reader tool's environment is built.
    pub async fn read(&self, name: &str) -> Value {
        let ns = &self.ns;
        // Warm the python environment through the direct call first.
        let _ = poll_until_ready(&self.client, &format!("{ns}.{name}"), json!({}), Duration::from_secs(30)).await;
        extract_structured(
            &self.client.tools_call("host.tool_call", json!({"name": name})).await.expect("tool_call"),
        )
    }
}

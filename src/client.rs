//! The streamable-HTTP MCP client (PRD-mcphost-call-cli-from-contract R2):
//! one JSON-RPC request over `POST <base>/mcp`, extracted from
//! `compat_check.rs` so the deploy gate and `mcphost call` speak through the
//! same transport.

use serde_json::{Value, json};

/// The stateless SEP-2575 `_meta` block every non-`initialize` request
/// needs, matching `tests/common/mod.rs`'s client.
pub fn with_client_meta(mut params: Value) -> Value {
    if let Some(obj) = params.as_object_mut() {
        obj.insert(
            "_meta".to_string(),
            json!({
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {},
            }),
        );
    }
    params
}

/// One JSON-RPC-over-streamable-HTTP request, bundled so [`rpc_call`] stays
/// under this repo's `too-many-arguments-threshold = 5`.
pub struct RpcRequest<'a> {
    pub method: &'a str,
    pub params: Value,
    pub id: u64,
    pub bearer: Option<&'a str>,
}

/// Why an [`rpc_call`] did not yield a `result`. `Display` is the exact
/// one-line detail `check-compat` has always reported.
#[derive(Debug)]
pub enum RpcFailure {
    /// The request never produced a response (connect, TLS, timeout).
    Unreachable(String),
    /// A response arrived but carried no JSON-RPC message -- `status` and
    /// the raw `body` are kept so a 5xx classifies as a host failure.
    Unparsed {
        status: u16,
        detail: String,
        body: String,
    },
    /// The JSON-RPC `error` member, verbatim, and the method that drew it.
    JsonRpc {
        method: String,
        status: u16,
        error: Value,
    },
}

impl std::fmt::Display for RpcFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RpcFailure::Unreachable(detail) | RpcFailure::Unparsed { detail, .. } => {
                f.write_str(detail)
            }
            RpcFailure::JsonRpc { method, error, .. } => {
                write!(f, "{method} returned a JSON-RPC error: {error}")
            }
        }
    }
}

pub async fn rpc_call(
    client: &reqwest::Client,
    base_url: &str,
    req: RpcRequest<'_>,
) -> Result<Value, RpcFailure> {
    let RpcRequest {
        method,
        params,
        id,
        bearer,
    } = req;
    let body_params = if method == "initialize" {
        params
    } else {
        with_client_meta(params)
    };
    let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": body_params});

    let mut http_req = client
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .json(&body);
    if method != "initialize" {
        http_req = http_req.header("Mcp-Method", method);
        if method == "tools/call"
            && let Some(name) = tool_call_name(&body)
        {
            http_req = http_req.header("Mcp-Name", name);
        }
    }
    if let Some(key) = bearer {
        http_req = http_req.header("Authorization", format!("Bearer {key}"));
    }

    let resp = http_req
        .send()
        .await
        .map_err(|e| RpcFailure::Unreachable(format!("{method} request failed: {e}")))?;
    let status = resp.status();
    // PRD-mcphost-one-next-tool requirement 3: a call that binds this
    // connection (`signup`) now emits notifications/tools/list_changed
    // before its own result, which upgrades that one response from
    // `application/json` to `text/event-stream` (`rmcp`'s own
    // `StreamableHttpServerConfig::json_response` fallback) -- the real
    // JSON-RPC message is always the last one on the wire.
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let unparsed = |detail: String, body: String| RpcFailure::Unparsed {
        status: status.as_u16(),
        detail,
        body,
    };
    let text = resp.text().await.map_err(|e| {
        unparsed(
            format!("{method} response ({status}) did not read as text: {e}"),
            String::new(),
        )
    })?;
    let parsed: Value = if content_type.starts_with("text/event-stream") {
        // PRD-mcphost-sse-frame-contract-generated R5: the result frame is
        // picked by the FRAME_SEQUENCES row for this request type, and a body
        // the row does not allow is a `frame_contract_violation`.
        let frames = crate::protocol::frames::split_sse_frames(&text);
        let request_type = crate::protocol::frames::request_type_of(method, bearer.is_some());
        crate::protocol::frames::select_result(&frames, request_type)
            .cloned()
            .map_err(|violation| unparsed(format!("{method} SSE response ({status}): {violation}"), text.clone()))?
    } else {
        serde_json::from_str(&text).map_err(|e| {
            unparsed(
                format!("{method} response ({status}) did not parse as JSON: {e}"),
                text.clone(),
            )
        })?
    };
    if let Some(error) = parsed.get("error") {
        return Err(RpcFailure::JsonRpc {
            method: method.to_string(),
            status: status.as_u16(),
            error: error.clone(),
        });
    }
    Ok(parsed.get("result").cloned().unwrap_or(Value::Null))
}

fn tool_call_name(body: &Value) -> Option<String> {
    body.get("params")
        .and_then(|p| p.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// `CallToolResult::structured` puts the value in `structuredContent`;
/// fall back to the first text content block, matching the test harness's
/// `extract_structured`.
pub fn extract_structured(call_result: &Value) -> Value {
    if let Some(sc) = call_result.get("structuredContent") {
        return sc.clone();
    }
    if let Some(content) = call_result.get("content").and_then(Value::as_array)
        && let Some(first) = content.first()
        && let Some(text) = first.get("text").and_then(Value::as_str)
    {
        return serde_json::from_str(text).unwrap_or(Value::Null);
    }
    Value::Null
}

/// PRD-mcphost-call-cli-from-contract R3: the one table from a host
/// `error_code` to a `mcphost call` exit code. Every arm of
/// `AppError::code()` is listed here explicitly (a test parses `errors.rs`
/// and fails on any code missing), so a new error class cannot silently
/// fall to the default.
///
/// 2 = the arguments were wrong, 3 = the tool (or its test) failed,
/// 4 = authentication or authorization, 5 = the host failed or was
/// unreachable, 1 = anything else the host refused.
pub const EXIT_CODE_TABLE: &[(&str, i32)] = &[
    ("args_invalid", 2),
    ("invalid_params", 2),
    ("invalid_spec", 2),
    ("invalid_tool_name", 2),
    ("unknown_kind", 2),
    ("spec_too_large", 2),
    ("too_many_tools", 2),
    ("tool_exception", 3),
    ("call_timeout", 3),
    ("secret_missing", 3),
    ("registry_rejected", 3),
    ("bearer_invalid", 4),
    ("tenant_key_missing", 4),
    ("tenant_key_invalid", 4),
    ("forbidden", 4),
    ("tenant_disabled", 4),
    ("invalid_token", 4),
    ("conflicting_credentials", 4),
    ("wrong_tenant", 4),
    (crate::scopes::INSUFFICIENT_SCOPE, 4),
    ("auth_conflict", 4),
    ("handoff_token_invalid", 4),
    ("handoff_token_redeemed", 4),
    ("handoff_token_expired", 4),
    ("storage", 5),
    ("internal", 5),
    ("service_unavailable", 5),
    ("rate_limited", 1),
    ("tool_not_found", 1),
    ("tenant_not_found", 1),
    ("group_not_found", 1),
    ("registry_disabled", 1),
    ("namespace_unverified", 1),
    ("share_quota_exceeded", 1),
    ("issuer_already_registered", 1),
    ("issuer_quota_exceeded", 1),
    ("issuer_not_found", 1),
    ("quota_trusted_issuers", 1),
];

/// Exit code for a host `error_code`; 1 for a code the table does not name
/// (host error codes are open-ended: `AppError::Structured` carries any).
pub fn exit_code_for(error_code: &str) -> i32 {
    EXIT_CODE_TABLE
        .iter()
        .find(|(code, _)| *code == error_code)
        .map_or(1, |(_, exit)| *exit)
}

/// A failed `tools/call`, ready to print: `{"error_code","message","data"}`
/// on stderr and exit with `exit`.
#[derive(Debug, Clone, PartialEq)]
pub struct CallError {
    pub exit: i32,
    pub error_code: String,
    pub message: String,
    pub data: Value,
}

impl CallError {
    pub fn envelope(&self) -> Value {
        json!({"error_code": self.error_code, "message": self.message, "data": self.data})
    }
}

/// Classify an [`RpcFailure`]: a JSON-RPC error by its `data.error_code`
/// (a bare 5xx without one is a host failure), an unreadable 5xx or an
/// unreachable host as exit 5, any other unreadable reply as exit 1.
pub fn classify(failure: &RpcFailure) -> CallError {
    match failure {
        RpcFailure::JsonRpc { status, error, .. } => {
            let data = error.get("data").cloned().unwrap_or(Value::Null);
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            match data.get("error_code").and_then(Value::as_str) {
                Some(code) => CallError {
                    exit: exit_code_for(code),
                    error_code: code.to_string(),
                    message,
                    data,
                },
                None if *status >= 500 => CallError {
                    exit: 5,
                    error_code: "host_unavailable".to_string(),
                    message,
                    data,
                },
                None => CallError {
                    exit: 1,
                    error_code: "jsonrpc_error".to_string(),
                    message,
                    data,
                },
            }
        }
        RpcFailure::Unparsed { status, detail, body } => CallError {
            exit: if *status >= 500 { 5 } else { 1 },
            error_code: if *status >= 500 { "host_unavailable" } else { "bad_response" }
                .to_string(),
            message: detail.clone(),
            data: json!({"status": status, "body": body.chars().take(512).collect::<String>()}),
        },
        RpcFailure::Unreachable(detail) => CallError {
            exit: 5,
            error_code: "host_unreachable".to_string(),
            message: detail.clone(),
            data: Value::Null,
        },
    }
}

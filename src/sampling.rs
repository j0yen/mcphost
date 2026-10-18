//! PRD-mcphost-sampling-from-tools: `mcphost.sampling.ask(prompt, max_tokens)`
//! from inside a sandboxed tool.
//!
//! The host forwards the ask as `sampling/createMessage` on the calling
//! connection's peer when that connection advertised `sampling` at
//! `initialize`; every other caller gets `sampling_unsupported` naming why
//! (`client`: the live connection did not advertise it; `no_client`: the run
//! has no connection at all, i.e. a job or trigger) and an alternative.
//!
//! The client channel follows `elicit.rs`: [`SAMPLING_CLIENT`] is set around
//! one `tools/call` dispatch by `McpHostHandler::call_tool` and is absent for
//! every context with no live client.
// rmcp 3.3.0 marks the whole sampling surface deprecated by SEP-2577; the
// bridge keeps working until it is removed (PRD "Migration / compatibility").
#![allow(deprecated)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use rmcp::RoleServer;
use rmcp::model::CreateMessageRequestParams;
use rmcp::service::Peer;
use serde_json::{Value, json};

use crate::db::Tenant;
use crate::err_code;
use crate::kinds::{CallLog, KindError, SamplingBackend};
use crate::state::AppState;

err_code! {
    /// A single run asked the client's model more times than its call ceiling.
    pub SAMPLING_CALLS_EXCEEDED,
    code: "sampling_calls_exceeded", family: Plan, class: PlanLimit, retryable: false,
    remedy: "Ask the client's model fewer times per run; data.limit is the per-run sampling-call ceiling.",
}

err_code! {
    /// The tenant's daily sampling token quota is spent.
    pub SAMPLING_QUOTA_EXCEEDED,
    code: "sampling_quota_exceeded", family: Plan, class: PlanLimit, retryable: false,
    remedy: "Wait for the daily reset or move to a plan with a larger sampling_tokens_per_day.",
}

err_code! {
    /// The client's model did not answer a sampling request in time.
    pub SAMPLING_TIMEOUT,
    code: "sampling_timeout", family: Call, class: AgentFault, retryable: true,
    remedy: "Retry with a smaller max_tokens or a shorter prompt; the client's model did not answer in time.",
}

err_code! {
    /// The connected client does not support `sampling/createMessage`.
    pub SAMPLING_UNSUPPORTED,
    code: "sampling_unsupported", family: Call, class: HostRefusal, retryable: false,
    remedy: "Do not retry; this client cannot sample. Use data.alternative or call from a client that supports sampling.",
}

/// What `sampling_unsupported` tells a tool to do instead.
pub const SAMPLING_ALTERNATIVE: &str = "publish a prompt-kind tool with your own key";

/// `max_tokens` default when the tool passes none.
pub const SAMPLING_MAX_TOKENS_DEFAULT: u64 = 256;

/// Every code `mcphost.sampling` raises; an uncaught one is the tool call's
/// own error code (`kinds::python::map_envelope_error`).
pub const SAMPLING_ERROR_CODES: &[&str] = &[SAMPLING_UNSUPPORTED, SAMPLING_TIMEOUT, SAMPLING_CALLS_EXCEEDED, SAMPLING_QUOTA_EXCEEDED];

/// Asks one tool call (and the calls it composes) may make.
pub const SAMPLING_CALLS_MAX: u32 = 4;

/// The wall one ask gets. An ask made with less than this left of the tool's
/// own budget could not finish inside it, so it fails fast instead of asking.
pub const SAMPLING_WALL: Duration = Duration::from_secs(30);

/// The live connection a `tools/call` can sample through. `peer` is `Some`
/// only when the connection advertised `capabilities.sampling`.
#[derive(Clone)]
pub struct SamplingClient {
    pub peer: Option<Peer<RoleServer>>,
}

tokio::task_local! {
    /// Set by `McpHostHandler::call_tool` for the duration of one dispatch;
    /// absent for jobs and every context with no live client.
    pub static SAMPLING_CLIENT: SamplingClient;
}

/// The client of the `tools/call` being dispatched, if it has one.
pub fn current_client() -> Option<SamplingClient> {
    SAMPLING_CLIENT.try_with(Clone::clone).ok()
}

fn unsupported(state: &AppState, reason: &str) -> KindError {
    let mut data = json!({"reason": reason, "alternative": SAMPLING_ALTERNATIVE});
    if let Some(url) = crate::help::help_url(&state.public_url, crate::capabilities::UNSUPPORTED_CODE) {
        data["help_url"] = json!(url);
    }
    KindError::structured_with(
        SAMPLING_UNSUPPORTED,
        format!("mcphost.sampling has no {reason} to ask; alternative: {SAMPLING_ALTERNATIVE}"),
        data,
    )
}

fn timeout(wall: Duration, remaining: Duration, asked: bool) -> KindError {
    KindError::structured_with(
        SAMPLING_TIMEOUT,
        if asked {
            format!("the client did not answer within {}s", wall.as_secs())
        } else {
            format!(
                "{}ms of this call is left and an ask gets {}s; the client was not asked",
                remaining.as_millis(),
                wall.as_secs()
            )
        },
        json!({"remaining_ms": remaining.as_millis() as u64, "wall_ms": wall.as_millis() as u64, "asked": asked}),
    )
}

fn exceeded() -> KindError {
    KindError::structured_with(
        SAMPLING_CALLS_EXCEEDED,
        format!("at most {SAMPLING_CALLS_MAX} sampling asks per call"),
        json!({"limit": SAMPLING_CALLS_MAX}),
    )
}

fn quota_exceeded(limit: i64, used: i64, reset_unix: i64) -> KindError {
    KindError::structured_with(
        SAMPLING_QUOTA_EXCEEDED,
        format!("this plan allows {limit} sampling tokens per day and {used} are spent; resets at unix {reset_unix}"),
        json!({"limit": limit, "used": used, "reset_unix": reset_unix, "field": "sampling_tokens_per_day"}),
    )
}

/// What an ask is admitted against before the client is asked: its
/// `max_tokens` plus a quarter of the prompt's bytes.
pub fn estimate_tokens(max_tokens: u64, prompt_len: usize) -> i64 {
    i64::try_from(max_tokens.saturating_add(prompt_len as u64 / 4)).unwrap_or(i64::MAX)
}

fn app(e: crate::errors::AppError) -> KindError {
    crate::handler::app_error_to_kind_error(e)
}

/// The backend of a context with no live client (a job run, a test context).
pub struct NoClientSampling {
    pub state: Option<Arc<AppState>>,
}

#[async_trait::async_trait]
impl SamplingBackend for NoClientSampling {
    async fn ask(&self, _args: Value, _remaining: Duration) -> Result<Value, KindError> {
        Err(match &self.state {
            Some(state) => unsupported(state, "no_client"),
            None => KindError::structured_with(
                SAMPLING_UNSUPPORTED,
                format!("mcphost.sampling has no no_client to ask; alternative: {SAMPLING_ALTERNATIVE}"),
                json!({"reason": "no_client", "alternative": SAMPLING_ALTERNATIVE}),
            ),
        })
    }
}

/// The real backend of a `tools/call` (or a job run, with `client: None`).
pub struct TenantSampling {
    pub state: Arc<AppState>,
    pub tenant: Tenant,
    pub client: Option<SamplingClient>,
    /// Asks made so far: one backend per call, so the cap is per call.
    pub calls: AtomicU32,
    /// The call's own log (`host.tool.logs`): one counts-only line per ask.
    pub log: Arc<dyn CallLog>,
    /// Successful asks and their tokens so far in this call.
    pub tally: std::sync::Mutex<(u32, i64)>,
}

impl TenantSampling {
    pub fn new(state: Arc<AppState>, tenant: Tenant, client: Option<SamplingClient>, log: Arc<dyn CallLog>) -> Self {
        Self { state, tenant, client, calls: AtomicU32::new(0), log, tally: std::sync::Mutex::new((0, 0)) }
    }

    /// R5: the line `host.tool.logs` shows for one ask -- `sampling: {asks,
    /// tokens}` for this call so far plus lengths and, on a refusal, the
    /// error code. Never the prompt or the answer.
    fn log_ask(&self, prompt_len: usize, outcome: &Result<Value, KindError>) {
        let mut detail = serde_json::Map::new();
        detail.insert("prompt_len".into(), json!(prompt_len));
        let mut tally = self.tally.lock().unwrap_or_else(|e| e.into_inner());
        match outcome {
            Ok(out) => {
                tally.0 += 1;
                tally.1 += out["tokens"].as_i64().unwrap_or(0);
                detail.insert("reply_len".into(), json!(out["text"].as_str().map_or(0, str::len)));
            }
            Err(KindError::Structured { code, .. }) => {
                detail.insert("error".into(), json!(code));
            }
            Err(_) => {
                detail.insert("error".into(), json!("sampling_error"));
            }
        }
        detail.insert("asks".into(), json!(tally.0));
        detail.insert("tokens".into(), json!(tally.1));
        self.log.log(&json!({"sampling": detail}).to_string());
    }
}

#[async_trait::async_trait]
impl SamplingBackend for TenantSampling {
    async fn ask(&self, args: Value, remaining: Duration) -> Result<Value, KindError> {
        let prompt_len = args.get("prompt").and_then(Value::as_str).map_or(0, str::len);
        let outcome = self.ask_inner(args, remaining).await;
        self.log_ask(prompt_len, &outcome);
        outcome
    }
}

impl TenantSampling {
    async fn ask_inner(&self, args: Value, remaining: Duration) -> Result<Value, KindError> {
        let peer = match &self.client {
            None => return Err(unsupported(&self.state, "no_client")),
            Some(SamplingClient { peer: None }) => return Err(unsupported(&self.state, "client")),
            Some(SamplingClient { peer: Some(peer) }) => peer,
        };
        if self.calls.fetch_add(1, Ordering::SeqCst) >= SAMPLING_CALLS_MAX {
            return Err(exceeded());
        }
        let prompt = args
            .get("prompt")
            .and_then(Value::as_str)
            .filter(|p| !p.trim().is_empty())
            .ok_or_else(|| KindError::InvalidArgs("mcphost.sampling.ask: `prompt` must be a non-empty string".into()))?;
        let max_tokens = args
            .get("max_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(SAMPLING_MAX_TOKENS_DEFAULT)
            .clamp(1, u64::from(u32::MAX));
        let mut params = json!({
            "messages": [{"role": "user", "content": {"type": "text", "text": prompt}}],
            "maxTokens": max_tokens,
        });
        if let Some(system) = args.get("system").and_then(Value::as_str) {
            params["systemPrompt"] = json!(system);
        }
        if let Some(hints) = args.get("hints").and_then(Value::as_array) {
            let hints: Vec<Value> = hints.iter().filter_map(Value::as_str).map(|h| json!({"name": h})).collect();
            params["modelPreferences"] = json!({"hints": hints});
        }
        let params: CreateMessageRequestParams = serde_json::from_value(params)
            .map_err(|e| KindError::InvalidArgs(format!("mcphost.sampling.ask: {e}")))?;
        // Admit against the plan's daily ceiling before the client is asked.
        let day_start = crate::state::utc_midnight_unix(crate::state::now_unix());
        let limit = self.state.plans.get(&self.tenant.plan).map_or(i64::MAX, |p| p.sampling_tokens_per_day);
        let used = self.state.db.sampling_tokens_on(self.tenant.id, day_start).await.map_err(app)?;
        if used.saturating_add(estimate_tokens(max_tokens, prompt.len())) > limit {
            return Err(quota_exceeded(limit, used, day_start + 86_400));
        }
        if remaining < SAMPLING_WALL {
            return Err(timeout(SAMPLING_WALL, remaining, false));
        }
        let result = match tokio::time::timeout(SAMPLING_WALL, peer.create_message(params)).await {
            Err(_) => return Err(timeout(SAMPLING_WALL, remaining, true)),
            Ok(result) => result.map_err(|e| KindError::Exec(format!("sampling/createMessage failed: {e}")))?,
        };
        let out = render_result(&serde_json::to_value(&result).unwrap_or(Value::Null), prompt.len());
        let tokens = out["tokens"].as_i64().unwrap_or(0);
        self.state.db.sampling_record(self.tenant.id, day_start, tokens).await.map_err(app)?;
        Ok(out)
    }
}

/// The tool-facing `{text, model, stop_reason, tokens}` of a
/// `sampling/createMessage` result (serialised, so the wire shape is read
/// rather than rmcp's non-exhaustive types).
fn render_result(result: &Value, prompt_len: usize) -> Value {
    let content = &result["content"];
    let blocks: Vec<&Value> = match content {
        Value::Array(items) => items.iter().collect(),
        Value::Null => Vec::new(),
        single => vec![single],
    };
    let text: String = blocks.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("");
    let tokens = reported_tokens(result).unwrap_or((text.len() + prompt_len).div_ceil(4) as u64);
    json!({
        "text": text,
        "model": result["model"],
        "stop_reason": result["stopReason"],
        "tokens": tokens,
    })
}

/// The client's own usage report, `_meta.usage.{inputTokens,outputTokens}`
/// (rmcp's typed result carries no `usage` field, so `_meta` is the one place
/// a client's count survives). `None` when the client reported neither.
fn reported_tokens(result: &Value) -> Option<u64> {
    let usage = &result["_meta"]["usage"];
    let input = usage["inputTokens"].as_u64();
    let output = usage["outputTokens"].as_u64();
    (input.is_some() || output.is_some()).then(|| input.unwrap_or(0) + output.unwrap_or(0))
}

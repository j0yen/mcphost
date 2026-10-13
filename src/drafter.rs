//! PRD-mcphost-first-tool-from-description: the host's one outbound LLM
//! client. `host.tool_from_description` hands a [`Drafter`] a prompt built
//! only from generated contract text (see [`build_prompt`]) and gets back a
//! python [`Draft`] -- a spec plus the example invocations to `spec_test` it
//! with. `DrafterConfig::from_env` follows `EmailConfig`'s shape
//! (`src/email.rs`); [`FakeDrafter`] replays `tests/fixtures/draft/<id>.json`
//! so no test touches the network.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::errors::AppError;
use crate::kinds::KindExample;
use crate::kinds::python::{BRIDGE_MODULES, unknown_import_hint};

/// One POST per draft, no longer.
pub const DRAFT_TIMEOUT: Duration = Duration::from_secs(30);

/// Most `invocations` a draft may propose (`spec_test`'s own cap is wider).
pub const MAX_DRAFT_INVOCATIONS: usize = 3;

/// Host-wide drafts per UTC day when `MCPHOST_DRAFT_DAILY_CAP` is unset.
pub const DEFAULT_DAILY_CAP: i64 = 200;

/// `$MCPHOST_DRAFT_DAILY_CAP`, read per call so an operator can lower it live.
pub fn daily_cap() -> i64 {
    std::env::var("MCPHOST_DRAFT_DAILY_CAP")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(DEFAULT_DAILY_CAP)
}

/// The tool-name rule `host.tool_publish` enforces; shown to the model
/// verbatim.
pub const TOOL_NAME_REGEX: &str = "^[a-z][a-z0-9_]{1,40}$";

/// The only non-generated text in a prompt: the reply shape the host parses.
/// Not sandbox prose -- the sandbox contract is entirely [`build_prompt`]'s
/// generated sections.
pub const RESPONSE_FORMAT: &str = "Reply with one JSON object and nothing else: \
{\"name\": <tool name>, \"spec\": <python spec>, \"invocations\": [<args object>, ...]}";

/// `$MCPHOST_DRAFT_API_URL`/`$MCPHOST_DRAFT_API_KEY`/`$MCPHOST_DRAFT_MODEL`;
/// an absent `api_url` is the whole "not configured" signal.
#[derive(Clone, Default)]
pub struct DrafterConfig {
    pub api_url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
}

impl DrafterConfig {
    pub fn from_env() -> Self {
        Self {
            api_url: std::env::var("MCPHOST_DRAFT_API_URL").ok(),
            api_key: std::env::var("MCPHOST_DRAFT_API_KEY").ok(),
            model: std::env::var("MCPHOST_DRAFT_MODEL").ok(),
        }
    }

    pub fn is_configured(&self) -> bool {
        self.api_url.is_some()
    }
}

/// A model's answer: the python spec, the invocations to test it with, and
/// the token counts the call cost.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Draft {
    #[serde(default)]
    pub name: Option<String>,
    pub spec: Value,
    #[serde(default)]
    pub invocations: Vec<Value>,
    #[serde(default)]
    pub prompt_tokens: i64,
    #[serde(default)]
    pub completion_tokens: i64,
}

#[async_trait::async_trait]
pub trait Drafter: Send + Sync {
    async fn draft(&self, prompt: &str) -> Result<Draft, AppError>;
}

/// Replays queued drafts in order and records every prompt it was given.
pub struct FakeDrafter {
    queue: Mutex<VecDeque<Draft>>,
    prompts: Mutex<Vec<String>>,
}

impl FakeDrafter {
    pub fn new(drafts: Vec<Draft>) -> Self {
        Self { queue: Mutex::new(drafts.into()), prompts: Mutex::new(Vec::new()) }
    }

    /// Loads `<dir>/<id>.json` (`{name, spec, invocations}`) as the one
    /// queued draft.
    pub fn from_fixture(dir: &std::path::Path, id: &str) -> Result<Self, String> {
        Ok(Self::new(vec![load_fixture(dir, id)?]))
    }

    pub fn prompts(&self) -> Vec<String> {
        self.prompts.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

pub fn load_fixture(dir: &std::path::Path, id: &str) -> Result<Draft, String> {
    let path = dir.join(format!("{id}.json"));
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

#[async_trait::async_trait]
impl Drafter for FakeDrafter {
    async fn draft(&self, prompt: &str) -> Result<Draft, AppError> {
        self.prompts.lock().unwrap_or_else(|e| e.into_inner()).push(prompt.to_string());
        self.queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front()
            .ok_or_else(|| AppError::Internal("FakeDrafter has no draft left".into()))
    }
}

/// Anthropic Messages API client: one POST to `api_url`, 30 s timeout.
pub struct HttpDrafter {
    client: reqwest::Client,
    config: DrafterConfig,
}

impl HttpDrafter {
    pub fn new(config: DrafterConfig) -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder().timeout(DRAFT_TIMEOUT).build()?;
        Ok(Self { client, config })
    }
}

#[async_trait::async_trait]
impl Drafter for HttpDrafter {
    async fn draft(&self, prompt: &str) -> Result<Draft, AppError> {
        let url = self
            .config
            .api_url
            .clone()
            .ok_or_else(|| AppError::Internal("drafter not configured".into()))?;
        let body = json!({
            "model": self.config.model.clone().unwrap_or_default(),
            "max_tokens": 2048,
            "messages": [{"role": "user", "content": prompt}],
        });
        let mut req = self.client.post(url).header("anthropic-version", "2023-06-01").json(&body);
        if let Some(key) = &self.config.api_key {
            req = req.header("x-api-key", key);
        }
        let resp = req.send().await.map_err(|e| AppError::Internal(format!("draft request: {e}")))?;
        if !resp.status().is_success() {
            return Err(AppError::Internal(format!("draft provider answered {}", resp.status())));
        }
        let body: Value = resp.json().await.map_err(|e| AppError::Internal(format!("draft body: {e}")))?;
        let text = body["content"][0]["text"].as_str().unwrap_or_default();
        let mut draft = parse_draft_text(text)
            .ok_or_else(|| AppError::Internal("draft reply is not the requested JSON".into()))?;
        draft.prompt_tokens = body["usage"]["input_tokens"].as_i64().unwrap_or(0);
        draft.completion_tokens = body["usage"]["output_tokens"].as_i64().unwrap_or(0);
        Ok(draft)
    }
}

/// The JSON object in a model reply, tolerating a ```json fence around it.
pub fn parse_draft_text(text: &str) -> Option<Draft> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    serde_json::from_str(text.get(start..=end)?).ok()
}

/// The `host.tool_from_description` prompt: labelled sections whose bodies
/// are all generated (the module list, `KindExample.spec`, the name regex)
/// or the caller's own words (description, and on retry the failure data as
/// JSON), plus [`RESPONSE_FORMAT`]. No other hand-written prose.
pub fn build_prompt(description: &str, example: &KindExample, retry: Option<(&Value, &Value)>) -> String {
    let mut sections: Vec<(&str, String)> = vec![
        ("sandbox_api", unknown_import_hint()),
        ("sandbox_modules", BRIDGE_MODULES.iter().map(|m| format!("mcphost.{}", m.name)).collect::<Vec<_>>().join("\n")),
        ("example_spec", example.spec.to_string()),
        ("example_call_args", example.call_args.to_string()),
        ("name_regex", TOOL_NAME_REGEX.to_string()),
        ("description", description.to_string()),
    ];
    if let Some((spec, error)) = retry {
        sections.push(("previous_spec", spec.to_string()));
        sections.push(("previous_error", error.to_string()));
    }
    let mut out = String::new();
    for (label, body) in sections {
        out.push_str(&format!("[{label}]\n{body}\n\n"));
    }
    out.push_str(RESPONSE_FORMAT);
    out
}

/// Whether a `host.spec_test` response counts as a passing draft: every
/// invocation `ok`, and at least one with a non-null output. `Err` carries
/// the first failing invocation (or a synthetic "all null" one).
pub fn spec_test_verdict(result: &Value) -> Result<(), Value> {
    let invocations = result["invocations"].as_array().cloned().unwrap_or_default();
    if let Some(bad) = invocations.iter().find(|i| i["ok"] != json!(true)) {
        return Err(bad.clone());
    }
    if !invocations.iter().any(|i| !i["output"].is_null()) {
        return Err(json!({"ok": false, "code": "no_output", "message": "no invocation returned a non-null result"}));
    }
    Ok(())
}

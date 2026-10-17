//! PRD-mcphost-elicitation-with-ask-fallback: `mcphost.elicit(message,
//! schema)` from inside a sandboxed tool.
//!
//! The accepted schema is [`ELICIT_SCHEMA_SUBSET`], one const: the bridge
//! docstring, `docs/sandbox-api.md`, [`validate_elicit_schema`] and the
//! mapping onto rmcp's [`PrimitiveSchemaDefinition`] all read it, so none of
//! them can list a type the others reject.
//!
//! Two channels answer an elicit, picked per call:
//! * `client` -- the connection advertised `elicitation.form` and can carry
//!   a server-to-client request, so the host sends `elicitation/create`
//!   ([`ClientChannel`], set around `tools/call` dispatch);
//! * `link` -- every other caller gets an ask-user question (`asks.rs`) and
//!   waits for the signed page's answer, or `{action: "pending", ...}`.
//!
//! Every elicit leaves one `asks` row whose `channel` says which.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use rmcp::model::{ElicitRequestParams, ElicitationAction, ElicitationSchema, PrimitiveSchemaDefinition};
use rmcp::service::{Peer, ServiceError};
use rmcp::RoleServer;
use serde_json::{Map, Value, json};

use crate::asks;
use crate::db::Tenant;
use crate::kinds::{ElicitBackend, KindError};
use crate::state::AppState;

/// The accepted schema subset: flat object, primitive properties only.
pub struct ElicitSchemaSubset {
    /// Allowed property types, in the order every rendering lists them.
    pub types: &'static [&'static str],
    pub max_properties: usize,
    pub string_max_length: usize,
    pub enum_max_values: usize,
}

pub const ELICIT_SCHEMA_SUBSET: ElicitSchemaSubset = ElicitSchemaSubset {
    types: &["string", "number", "integer", "boolean", "enum"],
    max_properties: 8,
    string_max_length: 1000,
    enum_max_values: 12,
};

/// Which rmcp [`PrimitiveSchemaDefinition`] variant each allowed type maps to.
/// A test asserts the type column equals [`ELICIT_SCHEMA_SUBSET`]`.types`.
pub const ELICIT_RMCP_VARIANTS: &[(&str, &str)] = &[
    ("string", "String"),
    ("number", "Number"),
    ("integer", "Integer"),
    ("boolean", "Boolean"),
    ("enum", "Enum"),
];

/// Elicits one tool call may make.
pub const ELICIT_CALLS_MAX: u32 = 3;

/// Every code `mcphost.elicit` raises; an uncaught one is the tool call's own
/// error code (`kinds::python::map_envelope_error`).
pub const ELICIT_ERROR_CODES: &[&str] =
    &["elicit_schema_invalid", "elicit_timeout", "elicit_unsupported", "elicit_calls_exceeded"];

/// `timeout_s` default for the typed path.
pub const ELICIT_TIMEOUT_DEFAULT_S: u64 = 30;

/// The variant name of `def`, as listed in [`ELICIT_RMCP_VARIANTS`].
pub fn rmcp_variant_name(def: &PrimitiveSchemaDefinition) -> &'static str {
    match def {
        PrimitiveSchemaDefinition::Enum(_) => "Enum",
        PrimitiveSchemaDefinition::String(_) => "String",
        PrimitiveSchemaDefinition::Number(_) => "Number",
        PrimitiveSchemaDefinition::Integer(_) => "Integer",
        PrimitiveSchemaDefinition::Boolean(_) => "Boolean",
        _ => "?",
    }
}

/// The subset as one line: the text the bridge docstring, `docs/sandbox-api.md`
/// and the mapping test all show.
pub fn render_subset() -> String {
    let s = &ELICIT_SCHEMA_SUBSET;
    format!(
        "types: {}; at most {} properties; string maxLength at most {}; enum at most {} values",
        s.types.join(", "),
        s.max_properties,
        s.string_max_length,
        s.enum_max_values
    )
}

pub const DOCS_START: &str = "<!-- mcphost-elicit-subset:start -->";
pub const DOCS_END: &str = "<!-- mcphost-elicit-subset:end -->";

/// The marked block `docs/sandbox-api.md` carries.
pub fn render_docs_block() -> String {
    let s = &ELICIT_SCHEMA_SUBSET;
    let mut out = format!("{DOCS_START}\n| rule | value |\n|---|---|\n");
    out.push_str(&format!("| types | {} |\n", s.types.join(", ")));
    out.push_str(&format!("| max properties | {} |\n", s.max_properties));
    out.push_str(&format!("| string maxLength | {} |\n", s.string_max_length));
    out.push_str(&format!("| enum values | {} |\n", s.enum_max_values));
    out.push_str(&format!("| elicits per call | {ELICIT_CALLS_MAX} |\n{DOCS_END}"));
    out
}

// ---- schema validation -------------------------------------------------------

/// One validated property.
#[derive(Debug, Clone)]
pub struct Prop {
    pub name: String,
    pub ty: &'static str,
    /// The property as the normalized JSON-Schema fragment rmcp parses.
    pub schema: Value,
    pub default: Option<Value>,
    pub enum_values: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ValidSchema {
    pub props: Vec<Prop>,
    pub required: Vec<String>,
    /// `schema._resume`: the ask a pending elicit left behind.
    pub resume: Option<String>,
}

fn invalid(path: &str, why: &str) -> KindError {
    KindError::structured_with(
        ELICIT_SCHEMA_INVALID,
        format!("{path}: {why}; allowed: {}", render_subset()),
        json!({"path": path, "allowed": ELICIT_SCHEMA_SUBSET.types, "limits": {
            "max_properties": ELICIT_SCHEMA_SUBSET.max_properties,
            "string_max_length": ELICIT_SCHEMA_SUBSET.string_max_length,
            "enum_max_values": ELICIT_SCHEMA_SUBSET.enum_max_values,
        }}),
    )
}

fn validate_prop(name: &str, raw: &Value) -> Result<Prop, KindError> {
    let path = format!("$.{name}");
    let obj = raw.as_object().ok_or_else(|| invalid(&path, "a property must be an object"))?;
    let enum_list = obj.get("enum").or_else(|| obj.get("values"));
    let declared = obj.get("type").and_then(Value::as_str);
    let ty: &'static str = match (declared, enum_list) {
        (Some("enum"), _) | (Some("string") | None, Some(_)) => "enum",
        (Some("string"), None) => "string",
        (Some("number"), _) => "number",
        (Some("integer"), _) => "integer",
        (Some("boolean"), _) => "boolean",
        (Some(other), _) => return Err(invalid(&path, &format!("type `{other}` is not allowed"))),
        (None, None) => return Err(invalid(&path, "a property needs a `type`")),
    };
    let mut out = Map::new();
    for key in ["title", "description"] {
        if let Some(v) = obj.get(key).filter(|v| v.is_string()) {
            out.insert(key.into(), v.clone());
        }
    }
    let mut enum_values = Vec::new();
    match ty {
        "enum" => {
            let list = enum_list
                .and_then(Value::as_array)
                .ok_or_else(|| invalid(&path, "an enum needs a list of values"))?;
            if list.is_empty() || list.len() > ELICIT_SCHEMA_SUBSET.enum_max_values {
                return Err(invalid(&path, "enum value count out of range"));
            }
            for v in list {
                enum_values.push(
                    v.as_str().ok_or_else(|| invalid(&path, "enum values must be strings"))?.to_string(),
                );
            }
            out.insert("type".into(), json!("string"));
            out.insert("enum".into(), json!(enum_values));
        }
        "string" => {
            out.insert("type".into(), json!("string"));
            if let Some(m) = obj.get("maxLength") {
                let m = m
                    .as_u64()
                    .filter(|m| *m as usize <= ELICIT_SCHEMA_SUBSET.string_max_length)
                    .ok_or_else(|| invalid(&path, "maxLength above the limit"))?;
                out.insert("maxLength".into(), json!(m));
            }
        }
        _ => {
            out.insert("type".into(), json!(ty));
            for key in ["minimum", "maximum"] {
                if let Some(v) = obj.get(key).filter(|v| v.is_number()) {
                    out.insert(key.into(), v.clone());
                }
            }
        }
    }
    let default = obj.get("default").cloned();
    if let Some(d) = &default {
        out.insert("default".into(), d.clone());
    }
    Ok(Prop { name: name.to_string(), ty, schema: Value::Object(out), default, enum_values })
}

/// Validates `schema` against [`ELICIT_SCHEMA_SUBSET`]. Accepts either the
/// bare `{name: {type: ...}}` map or `{type: "object", properties, required}`;
/// the reserved `_resume` key (a string ask id) is not a property.
pub fn validate_elicit_schema(schema: &Value) -> Result<ValidSchema, KindError> {
    let top = schema.as_object().ok_or_else(|| invalid("$", "the schema must be an object"))?;
    let full_form = top.get("type").and_then(Value::as_str) == Some("object");
    let resume = top.get("_resume").and_then(Value::as_str).map(str::to_string);
    let (props_map, required_list): (&Map<String, Value>, Option<&Vec<Value>>) = if full_form {
        let props = top
            .get("properties")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid("$.properties", "an object schema needs `properties`"))?;
        (props, top.get("required").and_then(Value::as_array))
    } else {
        (top, None)
    };
    let names: Vec<&String> = props_map.keys().filter(|k| !(k.as_str() == "_resume" && !full_form)).collect();
    if names.is_empty() {
        return Err(invalid("$", "the schema needs at least one property"));
    }
    if names.len() > ELICIT_SCHEMA_SUBSET.max_properties {
        return Err(invalid("$", &format!("{} properties, at most {}", names.len(), ELICIT_SCHEMA_SUBSET.max_properties)));
    }
    let mut props = Vec::new();
    for name in names {
        props.push(validate_prop(name, &props_map[name])?);
    }
    let required = match required_list {
        Some(list) => list.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        None => props.iter().filter(|p| p.default.is_none()).map(|p| p.name.clone()).collect(),
    };
    Ok(ValidSchema { props, required, resume })
}

impl ValidSchema {
    /// The JSON-Schema object rmcp's `requestedSchema` parses.
    pub fn requested_schema_json(&self) -> Value {
        let properties: Map<String, Value> = self.props.iter().map(|p| (p.name.clone(), p.schema.clone())).collect();
        json!({"type": "object", "properties": properties, "required": self.required})
    }

    /// The same schema as rmcp's type.
    pub fn to_rmcp(&self) -> Result<ElicitationSchema, KindError> {
        serde_json::from_value(self.requested_schema_json())
            .map_err(|e| invalid("$", &format!("rmcp refused the schema: {e}")))
    }

    /// What `accept` means on the link channel: each property's default, else
    /// `true` / its first enum value / `0` / `""`.
    pub fn default_content(&self) -> Value {
        let mut out = Map::new();
        for p in &self.props {
            let v = p.default.clone().unwrap_or_else(|| match p.ty {
                "boolean" => json!(true),
                "enum" => json!(p.enum_values.first()),
                "number" | "integer" => json!(0),
                _ => json!(""),
            });
            out.insert(p.name.clone(), v);
        }
        Value::Object(out)
    }

    /// Link-channel buttons: the enum values for a single-enum schema (when
    /// they fit an ask), else `[accept, decline]`.
    fn link_options(&self) -> Vec<String> {
        if let [p] = self.props.as_slice()
            && p.ty == "enum"
            && (asks::OPTIONS_MIN..=asks::OPTIONS_MAX).contains(&p.enum_values.len())
            && p.enum_values.iter().all(|v| v.chars().count() <= asks::OPTION_MAX_CHARS)
        {
            return p.enum_values.clone();
        }
        vec!["accept".into(), "decline".into()]
    }
}

// ---- backends --------------------------------------------------------------

/// The live client connection a `tools/call` can elicit through.
#[derive(Clone)]
pub struct ClientChannel {
    pub peer: Peer<RoleServer>,
}

tokio::task_local! {
    /// Set by `McpHostHandler::call_tool` for the duration of one dispatch;
    /// absent for jobs and every context with no live client.
    pub static CLIENT_CHANNEL: ClientChannel;
}

/// The channel of the `tools/call` being dispatched, if it has one.
pub fn current_channel() -> Option<ClientChannel> {
    CLIENT_CHANNEL.try_with(Clone::clone).ok()
}

fn unsupported(reason: &str) -> KindError {
    KindError::structured_with(
        ELICIT_UNSUPPORTED,
        format!("mcphost.elicit has no {reason}"),
        json!({"reason": reason}),
    )
}

fn exceeded() -> KindError {
    KindError::structured_with(
        ELICIT_CALLS_EXCEEDED,
        format!("at most {ELICIT_CALLS_MAX} elicits per call"),
        json!({"limit": ELICIT_CALLS_MAX}),
    )
}

/// The backend of a context with no live client (a job run, a test context).
pub struct NoClientElicit;

#[async_trait::async_trait]
impl ElicitBackend for NoClientElicit {
    async fn elicit(&self, args: Value, _remaining: Duration) -> Result<Value, KindError> {
        let schema = args.get("schema").cloned().unwrap_or(Value::Null);
        validate_elicit_schema(&schema)?;
        Err(unsupported("no_client"))
    }
}

/// The real backend of a `tools/call`: one per call, so `calls` is per call.
pub struct TenantElicit {
    pub state: Arc<AppState>,
    pub tenant: Tenant,
    pub channel: Option<ClientChannel>,
    pub calls: AtomicU32,
}

impl TenantElicit {
    pub fn new(state: Arc<AppState>, tenant: Tenant, channel: Option<ClientChannel>) -> Self {
        Self { state, tenant, channel, calls: AtomicU32::new(0) }
    }
}

#[async_trait::async_trait]
impl ElicitBackend for TenantElicit {
    async fn elicit(&self, args: Value, remaining: Duration) -> Result<Value, KindError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) >= ELICIT_CALLS_MAX {
            return Err(exceeded());
        }
        let message = args
            .get("message")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .ok_or_else(|| KindError::InvalidArgs("mcphost.elicit: `message` must be a non-empty string".into()))?
            .to_string();
        let schema = validate_elicit_schema(args.get("schema").unwrap_or(&Value::Null))?;
        let timeout_s = args.get("timeout_s").and_then(Value::as_u64);
        let req = Req { message: &message, schema: &schema, timeout_s, remaining };
        match &self.channel {
            Some(channel) => self.typed(channel, &req).await,
            None => self.link(&req).await,
        }
    }
}

/// One validated elicit request.
#[derive(Clone, Copy)]
struct Req<'a> {
    message: &'a str,
    schema: &'a ValidSchema,
    timeout_s: Option<u64>,
    remaining: Duration,
}

fn app(e: crate::errors::AppError) -> KindError {
    crate::handler::app_error_to_kind_error(e)
}

impl TenantElicit {
    async fn typed(&self, channel: &ClientChannel, req: &Req<'_>) -> Result<Value, KindError> {
        let Req { message, schema, timeout_s, remaining } = *req;
        let wall = Duration::from_secs(timeout_s.unwrap_or(ELICIT_TIMEOUT_DEFAULT_S)).min(remaining);
        let params = ElicitRequestParams::FormElicitationParams {
            meta: None,
            message: message.to_string(),
            requested_schema: schema.to_rmcp()?,
        };
        let result = match channel.peer.create_elicitation_with_timeout(params, Some(wall)).await {
            Ok(r) => r,
            Err(ServiceError::Timeout { .. }) => {
                return Err(KindError::structured_with(
                    ELICIT_TIMEOUT,
                    format!("the client did not answer within {}s", wall.as_secs()),
                    json!({"timeout_s": wall.as_secs()}),
                ));
            }
            Err(e) => return Err(KindError::Exec(format!("elicitation/create failed: {e}"))),
        };
        let (action, content) = match result.action {
            ElicitationAction::Accept => ("accept", Some(result.content.unwrap_or_else(|| json!({})))),
            ElicitationAction::Decline => ("decline", None),
            _ => ("cancel", None),
        };
        let choice = match &content {
            Some(c) => c.to_string(),
            None if action == "decline" => "declined".to_string(),
            None => "cancelled".to_string(),
        };
        asks::record_client_elicit(&self.state, &self.tenant, message, &schema.requested_schema_json(), &choice)
            .await
            .map_err(app)?;
        Ok(match content {
            Some(c) => json!({"action": action, "content": c}),
            None => json!({"action": action}),
        })
    }

    async fn link(&self, req: &Req<'_>) -> Result<Value, KindError> {
        let Req { message, schema, timeout_s, remaining } = *req;
        let ask_id = match &schema.resume {
            Some(id) => id.clone(),
            None => {
                let mut context = schema.requested_schema_json().to_string();
                while context.len() > asks::CONTEXT_MAX_BYTES {
                    context.pop();
                }
                let created = asks::create(
                    &self.state,
                    &self.tenant,
                    &json!({"question": message, "options": schema.link_options(), "context": context}),
                )
                .await
                .map_err(app)?;
                created["ask_id"].as_str().unwrap_or_default().to_string()
            }
        };
        // Leave room to answer before the tool's own deadline.
        let budget_s = (remaining.as_secs().saturating_sub(1)).max(1);
        let wait_s = (crate::longpoll::WAIT_MAX_S as u64).min(budget_s).min(timeout_s.unwrap_or(u64::MAX)).max(1);
        let waited = asks::wait(&self.state, &self.tenant, &json!({"ask_id": ask_id, "timeout_s": wait_s}))
            .await
            .map_err(app)?;
        if waited["status"] == "answered" {
            let choice = waited["choice"].as_str().unwrap_or_default();
            return Ok(link_answer(schema, choice));
        }
        let row = asks::find(&self.state, &ask_id).await.map_err(app)?;
        let row = row.ok_or_else(|| unsupported("unknown_ask"))?;
        Ok(json!({
            "action": "pending",
            "ask_id": ask_id,
            "url": asks::signed_ask_url(&self.state.public_url, &self.tenant_key_hash(), &ask_id, row.expires_unix),
            "expires_unix": row.expires_unix,
        }))
    }

    fn tenant_key_hash(&self) -> String {
        self.tenant.key_hash.clone()
    }
}

/// The tool-facing result of an answered link question.
fn link_answer(schema: &ValidSchema, choice: &str) -> Value {
    if let [p] = schema.props.as_slice()
        && p.ty == "enum"
        && p.enum_values.iter().any(|v| v == choice)
    {
        return json!({"action": "accept", "content": {p.name.clone(): choice}});
    }
    if choice == "decline" {
        return json!({"action": "decline"});
    }
    json!({"action": "accept", "content": schema.default_content()})
}

crate::err_code! {
    pub ELICIT_CALLS_EXCEEDED,
    code: "elicit_calls_exceeded", family: Call, class: AgentFault, retryable: false,
    remedy: "Ask for fewer elicitations per call; data.limit is the maximum.",
}

crate::err_code! {
    pub ELICIT_SCHEMA_INVALID,
    code: "elicit_schema_invalid", family: Args, class: AgentFault, retryable: false,
    remedy: "Use only the schema subset in data.allowed and stay within data.limits.",
}

crate::err_code! {
    pub ELICIT_TIMEOUT,
    code: "elicit_timeout", family: Call, class: AgentFault, retryable: true,
    remedy: "The client did not answer in time; call again and answer within data.timeout_s.",
}

crate::err_code! {
    pub ELICIT_UNSUPPORTED,
    code: "elicit_unsupported", family: Call, class: HostRefusal, retryable: false,
    remedy: "This client or context cannot elicit; read data.reason and ask through host.ask.create instead.",
}


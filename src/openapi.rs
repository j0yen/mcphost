//! PRD-mcphost-openapi-for-rest-facade R2: the response/request shapes of the
//! HTTP façade as `schemars` structs, keyed by the names
//! [`crate::http::routes::SchemaRef::Json`] uses. Each schema is rendered
//! standalone (subschemas inlined, no `$ref`), so the same value validates a
//! captured body directly and drops into `components.schemas` unchanged.
//!
//! A struct only lists keys the handler always emits; anything optional or
//! deep stays free-form (`serde_json::Value`) so the schema cannot claim more
//! than the host guarantees.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use axum::body::Body;
use axum::extract::State;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, ETAG, IF_NONE_MATCH};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use schemars::JsonSchema;
use schemars::generate::SchemaSettings;
use serde_json::{Map, Value, json};

use crate::api_contract::ContractSnapshot;
use crate::http::routes::{Auth, ROUTES, SchemaRef};
use crate::state::AppState;

/// The error object inside every façade error envelope. `code` and `message`
/// are the only required keys; `help_url` and the incident-banner fields ride
/// along when present.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct Error {
    /// Stable machine-readable code (`args_invalid`, `rate_limited`, ...).
    pub code: String,
    pub message: String,
    pub help_url: Option<String>,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct ErrorEnvelope {
    #[schemars(extend("const" = false))]
    pub ok: bool,
    pub error: Error,
}

/// The older `{error_code, error}` body some JSON routes answer 4xx/5xx with.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct LegacyError {
    pub error_code: String,
    pub error: String,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct RunResult {
    #[schemars(extend("const" = true))]
    pub ok: bool,
    pub run_id: String,
    /// The tool's own result; shape is the tool's.
    pub result: Value,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct RunAccepted {
    #[schemars(extend("const" = true))]
    pub ok: bool,
    pub run_id: String,
    /// Absolute URL to `GET` until the run is terminal.
    pub poll: String,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct Healthz {
    pub ok: bool,
    pub version: String,
    #[schemars(required)]
    pub git_sha: Option<String>,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct StatusComponent {
    pub name: String,
    pub state: String,
    #[schemars(required)]
    pub last_sample: Option<Value>,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct StatusFeed {
    /// `operational` | `degraded` | ...
    pub state: String,
    pub components: Vec<StatusComponent>,
    pub generated_at: i64,
    pub contract_version: String,
    pub contract_sha: String,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct ContractDocument {
    pub contract_version: u32,
    pub contract_sha: String,
    pub tools: Vec<Value>,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct Plan {
    pub name: String,
    pub description: String,
    pub price_usd_month: f64,
    pub calls_per_day: i64,
    pub tools_max: i64,
    pub secrets_max: i64,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct Plans {
    pub billing_mode: String,
    pub plans: Vec<Plan>,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct EventKind {
    pub kind: String,
    pub description: String,
    pub data_fields: Vec<String>,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct EventKinds {
    pub generated_from: String,
    pub kinds: Vec<EventKind>,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct Capabilities {
    pub version: String,
    /// Keyed by primitive name; each row carries `implemented`, `methods`, ...
    pub primitives: serde_json::Map<String, Value>,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct Catalog {
    pub tools: Vec<Value>,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct Jwk {
    pub kty: String,
    pub kid: String,
    pub alg: String,
    pub crv: String,
    pub x: String,
    pub y: String,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct Jwks {
    pub keys: Vec<Jwk>,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct Reach {
    pub host: String,
    pub ok: bool,
    pub served_at: i64,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct ProtectedResourceMetadata {
    pub resource: String,
    pub authorization_servers: Vec<String>,
    pub bearer_methods_supported: Vec<String>,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct AuthorizationServerMetadata {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
    pub response_types_supported: Vec<String>,
    pub grant_types_supported: Vec<String>,
    pub code_challenge_methods_supported: Vec<String>,
}

fn render<T: JsonSchema>() -> Value {
    let settings = SchemaSettings::draft2020_12().with(|s| {
        s.inline_subschemas = true;
        s.meta_schema = None;
    });
    let mut value = settings.into_generator().into_root_schema_for::<T>().to_value();
    if let Some(obj) = value.as_object_mut() {
        obj.remove("title");
    }
    value
}

/// Every named schema a [`crate::http::routes::SchemaRef::Json`] may point at.
pub fn schemas() -> BTreeMap<&'static str, Value> {
    let free_object = serde_json::json!({"type": "object"});
    let mut map = BTreeMap::new();
    map.insert("Error", render::<Error>());
    map.insert("ErrorEnvelope", render::<ErrorEnvelope>());
    map.insert("LegacyError", render::<LegacyError>());
    map.insert("RunResult", render::<RunResult>());
    map.insert("RunAccepted", render::<RunAccepted>());
    map.insert("Healthz", render::<Healthz>());
    map.insert("StatusFeed", render::<StatusFeed>());
    map.insert("ContractDocument", render::<ContractDocument>());
    map.insert("Plans", render::<Plans>());
    map.insert("EventKinds", render::<EventKinds>());
    map.insert("Capabilities", render::<Capabilities>());
    map.insert("Catalog", render::<Catalog>());
    map.insert("Jwks", render::<Jwks>());
    map.insert("Reach", render::<Reach>());
    map.insert("ProtectedResourceMetadata", render::<ProtectedResourceMetadata>());
    map.insert("AuthorizationServerMetadata", render::<AuthorizationServerMetadata>());
    map.insert("Incidents", serde_json::json!({"type": "array", "items": {"type": "object"}}));
    map.insert("FramesDocument", free_object.clone());
    map.insert("ServerCard", free_object.clone());
    map.insert("OpenObject", free_object.clone());
    map.insert(
        "OpenApiDocument",
        json!({"type": "object", "required": ["openapi", "info", "paths"], "properties": {"openapi": {"type": "string"}}}),
    );
    // Replaced by the contract's `host.tool.call` `args` schema in the served
    // document; the generic shape is all a standalone validator can know.
    map.insert("ToolArgs", free_object);
    map
}

/// The schema named by `name`; panics on an unknown name so a typo in
/// `ROUTES` fails the first test that touches it.
pub fn schema(name: &str) -> Value {
    schemas().remove(name).unwrap_or_else(|| panic!("ROUTES names unknown schema {name:?}"))
}

/// The contract's `host.tool.call` `args` schema: what `POST /x/{token}/{tool}`
/// takes as its body, minus the tool-specific properties (those are per tenant
/// tool and live behind `x-mcphost-tool-schema`).
fn tool_args_schema(contract: &ContractSnapshot) -> Value {
    let dump: Value = serde_json::from_slice(&contract.bytes).unwrap_or(Value::Null);
    let args = dump["tools"]
        .as_array()
        .and_then(|tools| tools.iter().find(|t| t["name"] == "host.tool.call"))
        .map(|t| t["input_schema"]["properties"]["args"].clone())
        .filter(|v| v.is_object())
        .unwrap_or_else(|| json!({"type": "object"}));
    let mut args = args;
    if let Some(obj) = args.as_object_mut() {
        obj.insert("x-mcphost-tool-schema".into(), json!("/contract.json"));
    }
    args
}

fn operation_id(method: &str, path: &str) -> String {
    let mut id = method.to_ascii_lowercase();
    for seg in path.split('/').filter(|s| !s.is_empty()) {
        id.push('_');
        let seg = seg.trim_start_matches('{').trim_end_matches('}');
        id.extend(seg.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }));
    }
    id
}

fn body_content(body: SchemaRef) -> Option<Value> {
    match body {
        SchemaRef::Json(name) => Some(json!({"application/json": {"schema": {"$ref": format!("#/components/schemas/{name}")}}})),
        SchemaRef::Html => Some(json!({"text/html": {"schema": {"type": "string"}}})),
        SchemaRef::Binary => Some(json!({"application/octet-stream": {"schema": {"type": "string", "format": "binary"}}})),
        SchemaRef::Empty => None,
    }
}

fn path_params(path: &str) -> Vec<&str> {
    path.split('/')
        .filter_map(|seg| seg.strip_prefix('{').and_then(|s| s.strip_suffix('}')))
        .collect()
}

/// Renders the OpenAPI 3.1 document from [`ROUTES`] and the contract.
pub fn document(contract: &ContractSnapshot) -> Value {
    let mut paths: BTreeMap<&str, Map<String, Value>> = BTreeMap::new();
    let mut tags: BTreeMap<&str, ()> = BTreeMap::new();
    for route in ROUTES {
        tags.insert(route.tag, ());
        let mut parameters: Vec<Value> = path_params(route.path)
            .into_iter()
            .map(|name| json!({"name": name, "in": "path", "required": true, "schema": {"type": "string"}}))
            .collect();
        parameters.extend(
            route
                .query
                .iter()
                .map(|name| json!({"name": name, "in": "query", "required": false, "schema": {"type": "string"}})),
        );
        let mut responses = Map::new();
        for (status, body) in route.responses {
            let mut response = Map::new();
            let reason = StatusCode::from_u16(*status).ok().and_then(|s| s.canonical_reason()).unwrap_or("Response");
            response.insert("description".into(), json!(reason));
            if let Some(content) = body_content(*body) {
                response.insert("content".into(), content);
            }
            responses.insert(status.to_string(), Value::Object(response));
        }
        let mut op = Map::new();
        op.insert("operationId".into(), json!(operation_id(route.method, route.path)));
        op.insert("tags".into(), json!([route.tag]));
        op.insert("summary".into(), json!(route.summary));
        if !parameters.is_empty() {
            op.insert("parameters".into(), Value::Array(parameters));
        }
        if let Some(request) = route.request.and_then(body_content) {
            op.insert("requestBody".into(), json!({"required": true, "content": request}));
        }
        op.insert("responses".into(), Value::Object(responses));
        let auth = match route.auth {
            Auth::Anonymous => "anonymous",
            Auth::Admin => "admin",
            Auth::Bearer => "bearer",
            Auth::Token => "token",
            Auth::Signature => "signature",
        };
        op.insert("x-mcphost-auth".into(), json!(auth));
        if matches!(route.auth, Auth::Admin | Auth::Bearer) {
            op.insert("security".into(), json!([{"bearerAuth": []}]));
        }
        paths
            .entry(route.path)
            .or_default()
            .insert(route.method.to_ascii_lowercase(), Value::Object(op));
    }

    let mut schemas: Map<String, Value> = schemas().into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    schemas.insert("ToolArgs".into(), tool_args_schema(contract));

    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "mcphost HTTP API",
            "version": env!("CARGO_PKG_VERSION"),
            "x-contract-version": contract.version,
            "description": "Every non-MCP HTTP route mcphost serves, generated from the router's own route table.",
        },
        "servers": [{"url": "https://mcphost.dev"}],
        "tags": tags.keys().map(|t| json!({"name": t})).collect::<Vec<_>>(),
        "paths": paths,
        "components": {
            "schemas": schemas,
            "securitySchemes": {"bearerAuth": {"type": "http", "scheme": "bearer"}},
        },
    })
}

/// Where the committed copy of the document lives (`mcphost gen openapi`).
pub const DEFAULT_PATH: &str = "docs/openapi.json";

/// The committed form of the document: pretty JSON plus a trailing newline.
pub fn render_committed() -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(&document(&ContractSnapshot::builtin())).expect("openapi document serializes");
    bytes.push(b'\n');
    bytes
}

/// `info.version` is the crate version, which `wm-build land` bumps after the
/// document was committed, so the drift check compares everything but it.
fn without_version(mut doc: Value) -> Value {
    if let Some(info) = doc.get_mut("info").and_then(Value::as_object_mut) {
        info.remove("version");
    }
    doc
}

/// Drift check: `Ok` when the file at `path` is the live document (ignoring
/// `info.version`), else an error naming the file and the first differing path.
pub fn check_committed(path: &std::path::Path) -> Result<(), String> {
    let name = path.display();
    let raw = std::fs::read_to_string(path).map_err(|e| format!("{name}: cannot read: {e}"))?;
    let committed: Value = serde_json::from_str(&raw).map_err(|e| format!("{name}: not JSON: {e}"))?;
    let live = without_version(document(&ContractSnapshot::builtin()));
    let committed = without_version(committed);
    if committed == live {
        return Ok(());
    }
    let differs = |a: &Value, b: &Value| a != b;
    let paths_a = committed.get("paths").and_then(Value::as_object);
    let paths_b = live.get("paths").and_then(Value::as_object);
    let first = match (paths_a, paths_b) {
        (Some(a), Some(b)) => a
            .keys()
            .chain(b.keys())
            .find(|k| differs(a.get(*k).unwrap_or(&Value::Null), b.get(*k).unwrap_or(&Value::Null)))
            .map(|k| format!("paths[{k}]")),
        _ => None,
    };
    Err(format!(
        "{name} is stale ({}); run `mcphost gen openapi`",
        first.unwrap_or_else(|| "components or info differ".to_string())
    ))
}

/// The document's serialized bytes and quoted-sha256 `ETag`, built once.
fn served(contract: &ContractSnapshot) -> &'static (Vec<u8>, String) {
    static SERVED: OnceLock<(Vec<u8>, String)> = OnceLock::new();
    SERVED.get_or_init(|| {
        let bytes = serde_json::to_vec(&document(contract)).expect("openapi document serializes");
        let etag = format!("\"{}\"", crate::api_contract::sha_hex(&bytes));
        (bytes, etag)
    })
}

/// `GET /openapi.json`, anonymous: the pre-serialized document, `ETag` = sha256
/// of the body, and a matching `If-None-Match` gets an empty 304.
pub async fn openapi_json_route(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let (bytes, etag) = served(&state.contract);
    let matches = headers
        .get(IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').map(|t| t.trim().trim_start_matches("W/")).any(|t| t == etag || t == "*"));
    let mut response = if matches {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        let mut r = Response::new(Body::from(bytes.clone()));
        r.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        r
    };
    if let Ok(value) = HeaderValue::from_str(etag) {
        response.headers_mut().insert(ETAG, value);
    }
    response.headers_mut().insert(CACHE_CONTROL, HeaderValue::from_static("max-age=300"));
    response
}

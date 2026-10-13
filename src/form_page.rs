//! PRD-mcphost-form-tool: a tool published with `ui: {form: true}` gets a
//! page (`/f/{token}/{tool}`) where the agent's user fills in the tool's
//! arguments.
//!
//! This file owns the two tables the rest of the feature is generated from:
//! [`UI_FIELDS`] (the `ui` spec field's sub-fields, rendered into the publish
//! schema and enforced by `kinds::check_unknown_spec_field`) and
//! [`FORM_INPUTS`] (JSON Schema type -> HTML input).

use serde_json::{Value, json};

/// One `ui` sub-field: `(name, json type, max length)`. `max` is a character
/// cap for strings and `None` for every other type.
pub const UI_FIELDS: &[(&str, &str, Option<usize>)] = &[
    ("form", "boolean", None),
    ("submit_label", "string", Some(40)),
    ("success_text", "string", Some(200)),
];

/// The `ui` property's schema in `host.tool_publish`'s per-kind `spec`
/// branches, generated from [`UI_FIELDS`].
pub fn ui_field_schema() -> Value {
    let properties: serde_json::Map<String, Value> = UI_FIELDS
        .iter()
        .map(|(name, ty, max)| {
            let mut schema = json!({"type": ty});
            if let Some(max) = max {
                schema["maxLength"] = json!(max);
            }
            (name.to_string(), schema)
        })
        .collect();
    json!({
        "type": "object",
        "description": "Optional presentation hints. `form: true` gives a url-shared tool a /f/ \
            page where a person fills in the arguments and the submission lands in your inbox.",
        "properties": properties,
        "additionalProperties": false,
    })
}

/// Marker `html_input` for a schema type the form cannot render.
pub const UNSUPPORTED_INPUT: &str = "unsupported";

/// `(schema_type, format_or_items, html_input)`.
///
/// `schema_type` is a JSON Schema `type` keyword value, or the pseudo-type
/// `enum` for a property carrying the `enum` keyword. The second column is
/// the `format` keyword for `string`, and the `items` type for `array`.
pub const FORM_INPUTS: &[(&str, Option<&str>, &str)] = &[
    ("string", None, "text"),
    ("string", Some("date"), "date"),
    ("string", Some("email"), "email"),
    ("integer", None, "number step=1"),
    ("number", None, "number"),
    ("boolean", None, "checkbox"),
    ("enum", None, "select"),
    ("array", Some("string"), "textarea"),
    ("object", None, UNSUPPORTED_INPUT),
    ("null", None, UNSUPPORTED_INPUT),
];

/// The drift check: `table` must have a row for every entry of `accepted`
/// (the types the jsonschema validator accepts) and no row whose type is
/// outside it.
pub fn check_form_inputs_drift(table: &[(&str, Option<&str>, &str)], accepted: &[String]) -> Result<(), String> {
    for ty in accepted {
        if !table.iter().any(|(t, _, _)| t == ty) {
            return Err(format!("validator accepts type '{ty}' but FORM_INPUTS has no row for it"));
        }
    }
    for (t, f, _) in table {
        if !accepted.iter().any(|a| a == t) {
            return Err(format!("FORM_INPUTS row ({t}, {f:?}) names a type the validator does not accept"));
        }
    }
    Ok(())
}

// ---- page ----------------------------------------------------------------

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;

use crate::public_tool::{ShareError, client_ip, not_found_response, rate_limited_response, resolve_share, tenant_paused_response};
use crate::state::AppState;

/// `https://<host>/f/<token>/<tool>`.
pub fn form_url(state: &AppState, token: &str, tool: &str) -> String {
    format!("{}/f/{}/{}", state.public_url.trim_end_matches('/'), token, tool)
}

/// R2: whether `spec` was published with `ui: {form: true}`.
pub fn has_form(spec: &Value) -> bool {
    spec.get("ui").and_then(|u| u.get("form")).and_then(Value::as_bool) == Some(true)
}

/// The `ui` sub-field `name` as a string, if the spec set it.
pub fn ui_str(spec: &Value, name: &str) -> Option<String> {
    spec.get("ui")?.get(name)?.as_str().map(str::to_string)
}

/// One rendered input, derived from one `args_schema` property.
pub struct FormField {
    pub name: String,
    /// The [`FORM_INPUTS`] `html_input` cell, e.g. `number step=1`.
    pub input: &'static str,
    /// The [`FORM_INPUTS`] `schema_type` cell, e.g. `integer`.
    pub schema_type: &'static str,
    pub required: bool,
    pub default: Option<Value>,
    pub description: Option<String>,
    pub choices: Vec<Value>,
}

/// The [`FORM_INPUTS`] row for `(schema_type, format_or_items)`: the exact
/// pair when a row has it, else the type's plain row.
fn lookup_input(ty: &str, detail: Option<&str>) -> Option<(&'static str, &'static str)> {
    let row = FORM_INPUTS
        .iter()
        .find(|(t, f, _)| *t == ty && f.is_some() && *f == detail)
        .or_else(|| FORM_INPUTS.iter().find(|(t, f, _)| *t == ty && f.is_none()))?;
    Some((row.0, row.2))
}

/// One field per `properties` entry of `schema` that the table can render
/// (properties with an `unsupported` row are left off the page).
pub fn form_fields(schema: &Value) -> Vec<FormField> {
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let Some(props) = schema.get("properties").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut fields = Vec::new();
    for (name, prop) in props {
        let choices = prop.get("enum").and_then(Value::as_array).cloned().unwrap_or_default();
        let ty = prop.get("type").and_then(Value::as_str).unwrap_or("string");
        let resolved = if !choices.is_empty() {
            lookup_input("enum", None)
        } else if ty == "array" {
            let items = prop.get("items").and_then(|i| i.get("type")).and_then(Value::as_str);
            lookup_input("array", items).filter(|(_, _)| items == Some("string"))
        } else {
            lookup_input(ty, prop.get("format").and_then(Value::as_str))
        };
        let Some((schema_type, input)) = resolved else { continue };
        if input == UNSUPPORTED_INPUT {
            continue;
        }
        fields.push(FormField {
            name: name.clone(),
            input,
            schema_type,
            required: required.contains(&name.as_str()),
            default: prop.get("default").cloned(),
            description: prop.get("description").and_then(Value::as_str).map(str::to_string),
            choices,
        });
    }
    fields
}

fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(value_text).collect::<Vec<_>>().join("\n"),
        other => other.to_string(),
    }
}

use crate::claim::html_escape;

/// What to put in a field on render: the submitted text on a re-render,
/// else the schema default.
pub struct FormValues<'a> {
    pub submitted: &'a [(String, String)],
    /// `(field name, message)` shown beside that field.
    pub errors: &'a [(String, String)],
    /// Goes in the hidden `_csrf` field.
    pub csrf: &'a str,
    /// Already-escaped HTML shown above the form (success or general error).
    pub banner: &'a str,
}

fn render_field(f: &FormField, values: &FormValues<'_>) -> String {
    let name = html_escape(&f.name);
    let submitted = values.submitted.iter().find(|(k, _)| k == &f.name).map(|(_, v)| v.clone());
    let current = submitted.clone().or_else(|| f.default.as_ref().map(value_text)).unwrap_or_default();
    let required = if f.required { " required" } else { "" };
    let control = match f.input {
        "select" => {
            let options: String = f
                .choices
                .iter()
                .map(|c| {
                    let text = value_text(c);
                    let selected = if text == current { " selected" } else { "" };
                    format!("<option value=\"{0}\"{selected}>{0}</option>", html_escape(&text))
                })
                .collect();
            format!("<select name=\"{name}\" id=\"f-{name}\"{required}>{options}</select>")
        }
        "textarea" => format!(
            "<textarea name=\"{name}\" id=\"f-{name}\" rows=\"4\"{required}>{}</textarea>",
            html_escape(&current)
        ),
        "checkbox" => {
            let on = if submitted.is_some() {
                submitted.as_deref() == Some("on")
            } else {
                f.default.as_ref().and_then(Value::as_bool).unwrap_or(false)
            };
            format!("<input type=\"checkbox\" name=\"{name}\" id=\"f-{name}\"{}>", if on { " checked" } else { "" })
        }
        other => {
            let mut parts = other.split(' ');
            let ty = parts.next().unwrap_or("text");
            let mut extra: String = parts.map(|a| match a.split_once('=') {
                Some((k, v)) => format!(" {k}=\"{v}\""),
                None => format!(" {a}"),
            }).collect();
            if f.schema_type == "number" {
                extra.push_str(" step=\"any\"");
            }
            format!(
                "<input type=\"{ty}\" name=\"{name}\" id=\"f-{name}\" value=\"{}\"{extra}{required}>",
                html_escape(&current)
            )
        }
    };
    let help = f
        .description
        .as_ref()
        .map(|d| format!("<small class=\"help\">{}</small>", html_escape(d)))
        .unwrap_or_default();
    let error = values
        .errors
        .iter()
        .find(|(k, _)| k == &f.name)
        .map(|(_, m)| format!("<div class=\"error\" role=\"alert\">{}</div>", html_escape(m)))
        .unwrap_or_default();
    format!("<p><label for=\"f-{name}\">{name}</label> {control}{help}{error}</p>")
}

/// The whole page.
pub fn render_page(tool: &str, spec: &Value, schema: &Value, values: &FormValues<'_>) -> String {
    let (csrf, banner) = (values.csrf, values.banner);
    let submit = ui_str(spec, "submit_label").unwrap_or_else(|| "Submit".to_string());
    let fields: String = form_fields(schema).iter().map(|f| render_field(f, values)).collect();
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <meta name=\"robots\" content=\"noindex\"><title>{title}</title>\
         <style>body{{font-family:system-ui,sans-serif;max-width:32rem;margin:2rem auto;padding:0 1rem}}\
         label{{display:block;font-weight:600}}input,select,textarea{{width:100%;box-sizing:border-box;padding:.4rem}}\
         input[type=checkbox]{{width:auto}}.help{{display:block;color:#555}}.error{{color:#b00020}}\
         .ok{{color:#0a6b2d;font-weight:600}}</style></head><body><h1>{title}</h1>{banner}\
         <form method=\"post\">{fields}<input type=\"hidden\" name=\"_csrf\" value=\"{csrf}\">\
         <button type=\"submit\">{submit}</button></form></body></html>",
        title = html_escape(tool),
        submit = html_escape(&submit),
        csrf = html_escape(csrf),
    )
}

/// R5: a hidden-field token valid for an hour, bound to the share token id:
/// `<issued unix>.<salted hash of "form|<token id>|<issued>">`.
pub fn mint_csrf(state: &AppState, token_id: &str) -> String {
    let now = crate::state::now_unix();
    format!("{now}.{}", state.secrets.salted_hash(&format!("form|{token_id}|{now}")))
}

pub fn csrf_valid(state: &AppState, token_id: &str, presented: &str) -> bool {
    let Some((ts, mac)) = presented.split_once('.') else { return false };
    let Ok(issued) = ts.parse::<i64>() else { return false };
    let now = crate::state::now_unix();
    if issued > now || now - issued > CSRF_TTL_SECONDS {
        return false;
    }
    let expected = state.secrets.salted_hash(&format!("form|{token_id}|{issued}"));
    crate::auth::constant_time_eq(expected.as_bytes(), mac.as_bytes())
}

pub const CSRF_TTL_SECONDS: i64 = 3600;

/// `GET /f/{token}/{tool}` (R2): 404 bytes unless the token resolves to a
/// url-shared tool published with `ui.form`.
pub async fn form_get(
    State(state): State<Arc<AppState>>,
    Path((token, tool)): Path<(String, String)>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if !state
        .url_rate_limiter
        .allow(&client_ip(&headers, peer), crate::state::PUBLIC_URL_RATE_LIMIT_PER_MINUTE)
    {
        return rate_limited_response();
    }
    let share = match resolve_share(&state, &token, &tool).await {
        Ok(share) => share,
        Err(ShareError::NotFound) => return not_found_response(),
        Err(ShareError::TenantPaused) => return tenant_paused_response(),
    };
    if !has_form(&share.tool_row.spec) {
        return not_found_response();
    }
    let Some(kind) = state.kinds.get(&share.tool_row.kind) else { return not_found_response() };
    let descriptor = kind.describe(&share.tool_row.spec);
    let csrf = mint_csrf(&state, &share.token_id);
    let values = FormValues { submitted: &[], errors: &[], csrf: &csrf, banner: "" };
    let body = render_page(&share.tool_row.name, &share.tool_row.spec, &descriptor.input_schema, &values);
    crate::claim::html_response(StatusCode::OK, body)
}

// ---- submit ----------------------------------------------------------------

/// Cap on the result excerpt shown after a successful submit (R4).
const RESULT_EXCERPT_BYTES: usize = 1024;
const DEFAULT_SUCCESS_TEXT: &str = "Submitted, thanks.";
const CSRF_FIELD: &str = "_csrf";

/// R4: turns the posted strings into the JSON args the schema describes,
/// per the [`FORM_INPUTS`] row of each field. A value that does not parse as
/// its row's type is passed through as the string it was, so the validator
/// (not this function) reports it.
pub fn coerce_form(fields: &[FormField], posted: &[(String, String)]) -> Value {
    let mut args = serde_json::Map::new();
    for f in fields {
        let raw = posted.iter().rev().find(|(k, _)| k == &f.name).map(|(_, v)| v.as_str());
        let value = match f.schema_type {
            "boolean" => Some(Value::Bool(raw == Some("on") || raw == Some("true"))),
            "array" => {
                let items: Vec<Value> = raw
                    .unwrap_or("")
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(|l| Value::String(l.to_string()))
                    .collect();
                if items.is_empty() && !f.required { None } else { Some(Value::Array(items)) }
            }
            _ => match raw.map(str::trim) {
                None | Some("") if !f.required => None,
                None => None,
                Some(text) => Some(match f.schema_type {
                    "integer" => text.parse::<i64>().map(Value::from).unwrap_or_else(|_| Value::String(text.to_string())),
                    "number" => text
                        .parse::<f64>()
                        .ok()
                        .filter(|n| n.is_finite())
                        .and_then(serde_json::Number::from_f64)
                        .map(Value::Number)
                        .unwrap_or_else(|| Value::String(text.to_string())),
                    "enum" => f.choices.iter().find(|c| value_text(c) == text).cloned().unwrap_or_else(|| Value::String(text.to_string())),
                    _ => Value::String(raw.unwrap_or(text).to_string()),
                }),
            },
        };
        if let Some(v) = value {
            args.insert(f.name.clone(), v);
        }
    }
    Value::Object(args)
}

/// Every schema violation in `args`, as `(field, message)`. The field is the
/// failing top-level argument, or for a missing required property the name
/// jsonschema's text quotes; `""` when no field can be named.
fn validation_errors(schema: &Value, fields: &[FormField], args: &Value) -> Vec<(String, String)> {
    let Ok(validator) = jsonschema::validator_for(schema) else { return Vec::new() };
    validator
        .iter_errors(args)
        .map(|e| {
            let err = crate::kinds::describe_args_error(&e);
            let field = err.argument.clone().unwrap_or_else(|| {
                fields
                    .iter()
                    .find(|f| err.raw.contains(&format!("\"{}\"", f.name)))
                    .map(|f| f.name.clone())
                    .unwrap_or_default()
            });
            (field, err.message())
        })
        .collect()
}

fn excerpt(v: &Value) -> String {
    let mut text = serde_json::to_string(v).unwrap_or_default();
    if text.len() > RESULT_EXCERPT_BYTES {
        let mut end = RESULT_EXCERPT_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push('…');
    }
    text
}

fn banner_error(message: &str) -> String {
    format!("<p class=\"error\" role=\"alert\">{}</p>", html_escape(message))
}

/// `POST /f/{token}/{tool}` (R4, R5).
pub async fn form_post(
    State(state): State<Arc<AppState>>,
    Path((token, tool)): Path<(String, String)>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    axum::Form(posted): axum::Form<Vec<(String, String)>>,
) -> Response {
    let ip = client_ip(&headers, peer);
    if !state.url_rate_limiter.allow(&ip, crate::state::PUBLIC_URL_RATE_LIMIT_PER_MINUTE) {
        return rate_limited_response();
    }
    let share = match resolve_share(&state, &token, &tool).await {
        Ok(share) => share,
        Err(ShareError::NotFound) => return not_found_response(),
        Err(ShareError::TenantPaused) => return tenant_paused_response(),
    };
    let spec = share.tool_row.spec.clone();
    if !has_form(&spec) {
        return not_found_response();
    }
    let Some(kind) = state.kinds.get(&share.tool_row.kind) else { return not_found_response() };
    let schema = kind.describe(&spec).input_schema;
    let name = share.tool_row.name.clone();
    // A fresh token on every render, so a re-rendered form can be resubmitted.
    let page = |status: StatusCode, submitted: &[(String, String)], errors: &[(String, String)], banner: &str| {
        let csrf = mint_csrf(&state, &share.token_id);
        let values = FormValues { submitted, errors, csrf: &csrf, banner };
        crate::claim::html_response(status, render_page(&name, &spec, &schema, &values))
    };

    let presented = posted.iter().find(|(k, _)| k == CSRF_FIELD).map(|(_, v)| v.as_str()).unwrap_or("");
    let submitted: Vec<(String, String)> = posted.iter().filter(|(k, _)| k != CSRF_FIELD).cloned().collect();
    if !csrf_valid(&state, &share.token_id, presented) {
        return page(StatusCode::FORBIDDEN, &submitted, &[], &banner_error("This form expired; check your entries and submit again."));
    }

    let fields = form_fields(&schema);
    let args = coerce_form(&fields, &submitted);
    let errors = validation_errors(&schema, &fields, &args);
    if !errors.is_empty() {
        let general: String = errors
            .iter()
            .filter(|(f, _)| !fields.iter().any(|x| &x.name == f))
            .map(|(_, m)| banner_error(m))
            .collect();
        return page(StatusCode::BAD_REQUEST, &submitted, &errors, &general);
    }

    let run_id = match crate::runs::enqueue_with_trigger(&state, &share.tenant, &name, args.clone(), ("form", &share.token_id)).await {
        Ok(id) => id,
        Err(e) => {
            return page(StatusCode::INTERNAL_SERVER_ERROR, &submitted, &[], &banner_error(&e.to_string()));
        }
    };

    let deadline = std::time::Instant::now() + state.public_url_sync_deadline;
    let run = loop {
        match state.db.get_run(run_id.clone(), share.tenant.id).await {
            Ok(Some(r)) if crate::public_tool::TERMINAL_STATUSES.contains(&r.status.as_str()) => break Some(r),
            Ok(Some(_)) if std::time::Instant::now() < deadline => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
            Ok(Some(_)) | Ok(None) | Err(_) => break None,
        }
    };

    // R4: the agent hears about the submission whether or not the run has
    // finished; the run id is the dedupe key, so a double-submit of the same
    // run cannot double-notify. Best-effort: the visitor's page does not fail
    // because the owner's inbox is full.
    let submission = json!({
        "kind": "form_submission",
        "tool": name,
        "args": args,
        "run_id": run_id,
        "ip_hash": crate::oauth_policy::ip_hash(&state, &ip),
    });
    let send_args = json!({
        "to": [share.tenant.namespace],
        "body": format!("form_submission: {name}"),
        "data": submission,
        "dedupe_key": run_id,
    });
    if let Err(e) = crate::messaging::send(&state, &share.tenant, &send_args).await {
        tracing::warn!(tool = %name, run_id = %run_id, error = %e, "form submission inbox write failed");
    }

    match run {
        Some(run) if run.status == "done" => {
            let result = crate::runs::attach_result(&state, share.tenant.id, &run, json!({})).await.ok().map(|m| m["result"].clone());
            let success = ui_str(&spec, "success_text").unwrap_or_else(|| DEFAULT_SUCCESS_TEXT.to_string());
            let mut banner = format!("<p class=\"ok\">{}</p>", html_escape(&success));
            if let Some(result) = result.filter(|r| !r.is_null()) {
                banner.push_str(&format!("<pre>{}</pre>", html_escape(&excerpt(&result))));
            }
            page(StatusCode::OK, &[], &[], &banner)
        }
        Some(_) => page(StatusCode::INTERNAL_SERVER_ERROR, &[], &[], &banner_error("Your submission was received but the tool failed to process it.")),
        None => page(StatusCode::ACCEPTED, &[], &[], "<p class=\"ok\">Received; it is still being processed.</p>"),
    }
}

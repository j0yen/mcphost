//! PRD-mcphost-resources-for-tenant-data: a tenant's tables, docs, run logs
//! and tool specs as MCP resources at `mcphost://` URIs.
//!
//! [`RESOURCE_TEMPLATES`] is the one registry: `resources/templates/list`,
//! `docs/protocol/resources.md` and the URI parser below are all read from
//! it. Every reader reuses the read path its `host.*` tool already has, so
//! there is no second query grammar and no second authorisation rule: the
//! caller (`handler.rs`) resolves the tenant exactly as `list_tools` does and
//! hands it in here.

use rmcp::model::{ReadResourceResult, Resource, ResourceContents};
use serde_json::{Value, json};

use crate::db::Tenant;
use crate::err_code;
use crate::errors::AppError;
use crate::state::AppState;

err_code! {
    /// A tenant already holds its plan's maximum number of open resource subscriptions.
    pub RESOURCES_MAX,
    code: "resources_max", family: Plan, class: PlanLimit, retryable: false,
    remedy: "Unsubscribe from a resource or move to a plan with a higher resources_max; data.limit is the plan maximum of open resource subscriptions.",
}

/// URI scheme every resource of this host carries.
pub const URI_SCHEME: &str = "mcphost://";

/// What a template's URI names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Table,
    Doc,
    Run,
    Tool,
}

impl ResourceKind {
    /// The kind's key in the generated contract's `resources` object.
    pub fn key(self) -> &'static str {
        match self {
            ResourceKind::Table => "table",
            ResourceKind::Doc => "doc",
            ResourceKind::Run => "run",
            ResourceKind::Tool => "tool",
        }
    }
}

/// One row of [`RESOURCE_TEMPLATES`].
#[derive(Debug)]
pub struct ResourceTemplate {
    pub kind: ResourceKind,
    /// RFC 6570 template, e.g. `mcphost://table/{name}`.
    pub uri_template: &'static str,
    pub name: &'static str,
    pub mime_type: &'static str,
    /// The `host.*` tool whose read path the reader reuses.
    pub reader: &'static str,
    /// The OAuth scope `resources/list` filters by and `resources/read`
    /// checks.
    pub scope: &'static str,
    /// The `resources/list` field carrying the object's human name, when the
    /// listing sets one (a doc's `title`); `None` for kinds listed without.
    pub title_field: Option<&'static str>,
}

pub const RESOURCE_TEMPLATES: &[ResourceTemplate] = &[
    ResourceTemplate {
        kind: ResourceKind::Table,
        uri_template: "mcphost://table/{name}",
        name: "table",
        mime_type: "application/json",
        reader: "host.table.query",
        scope: "tables:read",
        title_field: None,
    },
    ResourceTemplate {
        kind: ResourceKind::Doc,
        uri_template: "mcphost://doc/{id}",
        name: "doc",
        mime_type: "text/markdown",
        reader: "host.docs.get",
        scope: "docs:read",
        title_field: Some("title"),
    },
    ResourceTemplate {
        kind: ResourceKind::Run,
        uri_template: "mcphost://run/{id}/log",
        name: "run_log",
        mime_type: "text/plain",
        reader: "host.runs.get",
        scope: "runs:read",
        title_field: None,
    },
    ResourceTemplate {
        kind: ResourceKind::Tool,
        uri_template: "mcphost://tool/{name}/spec",
        name: "tool_spec",
        mime_type: "application/json",
        reader: "host.tool.list",
        scope: "tools:read",
        title_field: None,
    },
];

/// The name of the placeholder in `template`'s URI, e.g. `name` for
/// `mcphost://table/{name}`.
fn placeholder(template: &ResourceTemplate) -> &'static str {
    let rest = template.uri_template.split_once('{').map_or("", |(_, r)| r);
    rest.split_once('}').map_or("", |(p, _)| p)
}

/// How `template` formats a listed entry's `name`, placeholder unfilled:
/// `table:{name}`. [`list_entry`] fills the placeholder with the same key it
/// passes to [`uri_for`], and the contract's `name_field` renders this.
pub fn name_format(template: &ResourceTemplate) -> String {
    format!("{}:{{{}}}", template.name, placeholder(template))
}

/// The `resources` section of `contracts/host-tools.v1.json`, built by
/// iterating `templates` (the contract is the registry rendered): per kind,
/// the full URI template in `uri_scheme` and the `name` format in
/// `name_field`; `title_field` names the listing field holding the human
/// name, for the kinds that set one.
pub fn contract_section(templates: &[ResourceTemplate]) -> Value {
    let mut uri_scheme = serde_json::Map::new();
    let mut name_field = serde_json::Map::new();
    let mut title_field = serde_json::Map::new();
    for t in templates {
        uri_scheme.insert(t.kind.key().to_string(), json!(t.uri_template));
        name_field.insert(t.kind.key().to_string(), json!(name_format(t)));
        if let Some(title) = t.title_field {
            title_field.insert(t.kind.key().to_string(), json!(title));
        }
    }
    json!({"name_field": name_field, "title_field": title_field, "uri_scheme": uri_scheme})
}

/// The `resources.<field>.<kind>` paths where `committed`'s `resources`
/// section differs from what `templates` render (`resources` alone when the
/// committed file has no section), for `contract dump --check` to name.
pub fn contract_drift(committed: &Value, templates: &[ResourceTemplate]) -> Vec<String> {
    let live = contract_section(templates);
    let Some(have) = committed.get("resources").and_then(Value::as_object) else {
        return vec!["resources".to_string()];
    };
    let mut out = Vec::new();
    for (field, want) in live.as_object().into_iter().flatten() {
        let want = want.as_object().expect("section fields are objects");
        let got = have.get(field).and_then(Value::as_object);
        let mut kinds: Vec<&String> = want.keys().chain(got.into_iter().flat_map(|g| g.keys())).collect();
        kinds.sort();
        kinds.dedup();
        for kind in kinds {
            if want.get(kind) != got.and_then(|g| g.get(kind)) {
                out.push(format!("resources.{field}.{kind}"));
            }
        }
    }
    out
}

/// The template for `kind`.
pub fn template_for(kind: ResourceKind) -> &'static ResourceTemplate {
    RESOURCE_TEMPLATES
        .iter()
        .find(|t| t.kind == kind)
        .expect("every ResourceKind has a template row")
}

/// A URI parsed against the templates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedUri {
    pub kind: ResourceKind,
    pub key: String,
}

/// Parse `uri` against [`RESOURCE_TEMPLATES`]; `None` if it matches none.
pub fn parse_uri(uri: &str) -> Option<ParsedUri> {
    let rest = uri.strip_prefix(URI_SCHEME)?;
    let segments: Vec<&str> = rest.split('/').collect();
    let (kind, key) = match segments.as_slice() {
        ["table", name] => (ResourceKind::Table, name),
        ["doc", id] => (ResourceKind::Doc, id),
        ["run", id, "log"] => (ResourceKind::Run, id),
        ["tool", name, "spec"] => (ResourceKind::Tool, name),
        _ => return None,
    };
    if key.is_empty() {
        return None;
    }
    Some(ParsedUri { kind, key: (*key).to_string() })
}

/// The canonical URI of `kind` + `key`.
pub fn uri_for(kind: ResourceKind, key: &str) -> String {
    match kind {
        ResourceKind::Table => format!("{URI_SCHEME}table/{key}"),
        ResourceKind::Doc => format!("{URI_SCHEME}doc/{key}"),
        ResourceKind::Run => format!("{URI_SCHEME}run/{key}/log"),
        ResourceKind::Tool => format!("{URI_SCHEME}tool/{key}/spec"),
    }
}

/// Why a read did not produce contents.
#[derive(Debug)]
pub enum ReadError {
    /// Unknown URI, or an object the tenant does not own: `-32002`, never
    /// `forbidden`, so existence leaks nothing.
    NotFound(String),
    /// The caller's OAuth scope does not cover the template (`scope` is the
    /// word it lacks).
    Forbidden { scope: &'static str },
    App(AppError),
}

impl From<AppError> for ReadError {
    fn from(e: AppError) -> Self {
        ReadError::App(e)
    }
}

/// Rows `mcphost://table/{name}` returns before the byte ceiling cuts it.
pub const TABLE_READ_ROWS: i64 = 1000;

/// A body and whether it was cut to fit the byte ceiling.
struct Body {
    text: String,
    truncated: bool,
}

impl Body {
    fn whole(text: String) -> Self {
        Self { text, truncated: false }
    }
}

fn text_contents(uri: &str, mime: &str, body: Body) -> ReadResourceResult {
    let mut contents = ResourceContents::text(body.text, uri).with_mime_type(mime);
    let mut result_meta = None;
    if body.truncated {
        let mut meta = rmcp::model::MetaObject::new();
        meta.0.insert("truncated".to_string(), json!(true));
        contents = contents.with_meta(meta.clone());
        result_meta = Some(meta);
    }
    let mut result = ReadResourceResult::new(vec![contents]);
    result.meta = result_meta;
    result
}

/// Serialise a `{rows: [...], ...}` table body, keeping the longest prefix
/// of `rows` whose whole JSON is at most `max_bytes` (the cut falls at the
/// last complete row; a body with no rows is returned as is).
fn table_body(mut out: Value, max_bytes: usize) -> Result<Body, AppError> {
    let ser = |v: &Value| serde_json::to_string(v).map_err(|e| AppError::Internal(format!("serialise table resource: {e}")));
    let whole = ser(&out)?;
    if whole.len() <= max_bytes {
        return Ok(Body::whole(whole));
    }
    let rows = match out.get_mut("rows").map(Value::take) {
        Some(Value::Array(rows)) => rows,
        _ => return Ok(Body { text: whole, truncated: false }),
    };
    out["rows"] = Value::Array(Vec::new());
    // Bytes the body takes with an empty `rows`; each kept row adds its own
    // JSON plus a separating comma (none for the first).
    let mut used = ser(&out)?.len();
    let mut kept = 0;
    for row in &rows {
        let add = ser(row)?.len() + usize::from(kept > 0);
        if used + add > max_bytes {
            break;
        }
        used += add;
        kept += 1;
    }
    out["rows"] = Value::Array(rows.into_iter().take(kept).collect());
    Ok(Body { text: ser(&out)?, truncated: true })
}

/// Cut `text` at the last line break within `max_bytes`.
fn text_body(text: String, max_bytes: usize) -> Body {
    if text.len() <= max_bytes {
        return Body::whole(text);
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let end = text[..end].rfind('\n').map_or(end, |i| i + 1);
    Body { text: text[..end].to_string(), truncated: true }
}

/// A tool-style error that means "this object is not here for this tenant".
fn is_missing(e: &AppError) -> bool {
    matches!(
        e,
        AppError::Structured { code: crate::tables::TABLE_NOT_FOUND | crate::docs::DOCS_NOT_FOUND | crate::runs::RUN_NOT_FOUND, .. } | AppError::ToolNotFound(_)
    )
}

/// `resources/read`: the contents of `uri` for `tenant`. `token_scope` is the
/// OAuth scope claim (`None` for a key, which is unrestricted).
pub async fn read(
    state: &AppState,
    tenant: &Tenant,
    token_scope: Option<&str>,
    uri: &str,
) -> Result<ReadResourceResult, ReadError> {
    let not_found = || ReadError::NotFound(format!("no resource at {uri}"));
    let parsed = parse_uri(uri).ok_or_else(not_found)?;
    let template = template_for(parsed.kind);
    let scoped = [template.scope.to_string()];
    if !crate::oauth::scope_satisfied(token_scope, &scoped) {
        return Err(ReadError::Forbidden { scope: template.scope });
    }
    let max_bytes = state
        .plans
        .get(&tenant.plan)
        .map_or(1_048_576, |p| p.resource_read_bytes_max.max(0) as usize);
    let body = match parsed.kind {
        ResourceKind::Table => {
            if !crate::tables::table_exists(state, tenant, &parsed.key).await? {
                return Err(not_found());
            }
            let sql = format!("SELECT * FROM {}", crate::tables::quote_ident(&parsed.key));
            let args = json!({"sql": sql, "limit": TABLE_READ_ROWS});
            let out = crate::tables::table_query(state, tenant, &args, None).await?;
            table_body(out, max_bytes)?
        }
        ResourceKind::Doc => {
            let args = json!({"id": parsed.key, "text": true});
            let doc = crate::docs::doc_get(state, tenant, &args).await.map_err(|e| {
                if is_missing(&e) { not_found() } else { ReadError::App(e) }
            })?;
            text_body(doc_text(&doc), max_bytes)
        }
        ResourceKind::Run => {
            let args = json!({"run_id": parsed.key});
            let run = crate::runs::get(state, tenant, &args).await.map_err(|e| {
                if is_missing(&e) { not_found() } else { ReadError::App(e) }
            })?;
            text_body(run_log_text(&run), max_bytes)
        }
        ResourceKind::Tool => {
            let row = state
                .db
                .get_tool(tenant.id, parsed.key.clone())
                .await?
                .ok_or_else(not_found)?;
            let spec = serde_json::to_string(&json!({"name": row.name, "kind": row.kind, "spec": row.spec}))
                .map_err(|e| AppError::Internal(format!("serialise tool spec resource: {e}")))?;
            // A JSON object cannot be cut mid-way and stay JSON: a spec is
            // small by construction, so it is served whole.
            Body::whole(spec)
        }
    };
    Ok(text_contents(uri, template.mime_type, body))
}

/// A document's text: the extracted `text` when the store has one, else the
/// UTF-8 decode of its bytes.
fn doc_text(doc: &Value) -> String {
    if let Some(t) = doc.get("text").and_then(Value::as_str) {
        return t.to_string();
    }
    use base64::Engine as _;
    doc.get("content_base64")
        .and_then(Value::as_str)
        .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}

/// A run's log as plain text: status header, the error if any, and the
/// result when `host.runs.get` attached one. A snapshot -- a subscription
/// says when to read again.
fn run_log_text(run: &Value) -> String {
    let field = |k: &str| run.get(k).filter(|v| !v.is_null());
    let mut out = format!(
        "run {} tool={} status={}\n",
        run["run_id"].as_str().unwrap_or("?"),
        run["tool"].as_str().unwrap_or("?"),
        run["status"].as_str().unwrap_or("?"),
    );
    if let Some(ms) = field("duration_ms") {
        out.push_str(&format!("duration_ms={ms}\n"));
    }
    if let Some(err) = field("error") {
        out.push_str(&format!("error: {err}\n"));
    }
    if let Some(progress) = field("progress") {
        out.push_str(&format!("progress: {progress}\n"));
    }
    if let Some(result) = field("result") {
        out.push_str(&format!("result: {result}\n"));
    }
    out
}

/// A `resources/list` entry.
pub fn list_entry(kind: ResourceKind, key: &str, title: Option<&str>) -> Resource {
    let template = template_for(kind);
    let mut r = Resource::new(uri_for(kind, key), name_format(template).replace(&format!("{{{}}}", placeholder(template)), key)).with_mime_type(template.mime_type);
    if let (Some(_), Some(t)) = (template.title_field, title) {
        r = r.with_title(t);
    }
    r
}

/// `resources/list` for `tenant`: one entry per table, doc, run of the last
/// 24 h, and published tool, each kept only if `token_scope` covers its
/// template.
pub async fn list(state: &AppState, tenant: &Tenant, token_scope: Option<&str>) -> Result<Vec<Resource>, AppError> {
    let allowed = |kind: ResourceKind| crate::oauth::scope_satisfied(token_scope, &[template_for(kind).scope.to_string()]);
    let mut out = Vec::new();
    if allowed(ResourceKind::Table) {
        let tables = crate::tables::table_list(state, tenant, &json!({})).await?;
        for t in tables["tables"].as_array().into_iter().flatten() {
            if let Some(name) = t["name"].as_str() {
                out.push(list_entry(ResourceKind::Table, name, None));
            }
        }
    }
    if allowed(ResourceKind::Doc) {
        let docs = crate::docs::doc_list(state, tenant, &json!({"limit": 200})).await?;
        for d in docs["documents"].as_array().into_iter().flatten() {
            if let Some(id) = d["id"].as_str() {
                out.push(list_entry(ResourceKind::Doc, id, d["name"].as_str()));
            }
        }
    }
    if allowed(ResourceKind::Run) {
        let since = crate::state::now_unix() - 24 * 3600;
        let runs = crate::runs::list(state, tenant, &json!({"limit": 200})).await?;
        for r in runs["runs"].as_array().into_iter().flatten() {
            let recent = r["started_unix"].as_i64().is_none_or(|s| s >= since);
            if recent && let Some(id) = r["run_id"].as_str() {
                out.push(list_entry(ResourceKind::Run, id, r["tool"].as_str()));
            }
        }
    }
    if allowed(ResourceKind::Tool) {
        for row in state.db.list_tools(tenant.id).await? {
            out.push(list_entry(ResourceKind::Tool, &row.name, None));
        }
    }
    Ok(out)
}

/// Opening marker of the generated table in `docs/protocol/resources.md`.
pub const RESOURCES_TABLE_START: &str = "<!-- resources:start -->";
/// Closing marker of the generated table.
pub const RESOURCES_TABLE_END: &str = "<!-- resources:end -->";

/// The marked table of [`RESOURCE_TEMPLATES`] that `docs/protocol/resources.md`
/// carries byte-for-byte (`mcphost resources-doc` prints the whole file).
pub fn render_resources_table() -> String {
    let mut out = String::new();
    out.push_str(RESOURCES_TABLE_START);
    out.push('\n');
    out.push_str("| uriTemplate | mimeType | reader | scope |\n");
    out.push_str("|---|---|---|---|\n");
    for t in RESOURCE_TEMPLATES {
        out.push_str(&format!("| `{}` | `{}` | `{}` | `{}` |\n", t.uri_template, t.mime_type, t.reader, t.scope));
    }
    out.push_str(RESOURCES_TABLE_END);
    out
}

/// The whole of `docs/protocol/resources.md`.
pub fn render_resources_doc() -> String {
    format!(
        "# MCP resources\n\n\
         Generated by `mcphost resources-doc` from `RESOURCE_TEMPLATES` -- the same rows \
         `resources/templates/list` answers; do not hand-edit the table. A resource is read \
         with the credential the connection already carries (`/u/{{secret}}/mcp` path, \
         `Authorization` header, or the session's signup binding); an unknown URI, or an \
         object another tenant owns, is `-32002 resource_not_found`.\n\n{}\n",
        render_resources_table()
    )
}

/// The `resources/templates/list` rows, one per [`RESOURCE_TEMPLATES`] entry.
pub fn template_list() -> Vec<rmcp::model::ResourceTemplate> {
    RESOURCE_TEMPLATES
        .iter()
        .map(|t| rmcp::model::ResourceTemplate::new(t.uri_template, t.name).with_mime_type(t.mime_type))
        .collect()
}

/// One open `resources/subscribe`.
pub struct Subscription {
    pub tenant_id: i64,
    pub uri: String,
    /// The connection that opened it (`conn_key` in `handler.rs`).
    pub conn: String,
    pub peer: rmcp::service::Peer<rmcp::RoleServer>,
}

/// Both halves of the registry behind one lock.
#[derive(Default)]
struct Registry {
    /// Open `resources/subscribe` entries (counted against `resources_max`).
    subs: Vec<Subscription>,
    /// Connections that listed resources: they hear `list_changed` (not
    /// counted, `uri` unused).
    watchers: Vec<Subscription>,
}

impl Registry {
    /// Drop every entry whose connection has closed.
    fn prune(&mut self) {
        self.subs.retain(|s| !s.peer.is_transport_closed());
        self.watchers.retain(|s| !s.peer.is_transport_closed());
    }
}

/// PRD-mcphost-resources-for-tenant-data requirement 4: the in-memory
/// `(tenant, uri, peer)` map. Per process and lost on restart -- a client
/// re-subscribes on reconnect, as the spec has it. Cheap to clone: every
/// clone shares one map, so a spawned sender can drop a dead peer itself.
#[derive(Clone, Default)]
pub struct Subscriptions(std::sync::Arc<std::sync::Mutex<Registry>>);

impl Subscriptions {
    fn lock(&self) -> std::sync::MutexGuard<'_, Registry> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Open subscriptions `tenant_id` holds on live connections.
    pub fn open_for(&self, tenant_id: i64) -> usize {
        let mut reg = self.lock();
        reg.prune();
        reg.subs.iter().filter(|s| s.tenant_id == tenant_id).count()
    }

    /// Register `sub`; `Err(open)` when the tenant already holds `max`
    /// subscriptions and this one is new (re-subscribing the same URI on the
    /// same connection is idempotent and never counts twice).
    pub fn add(&self, sub: Subscription, max: i64) -> Result<(), usize> {
        let mut reg = self.lock();
        reg.prune();
        if reg.subs.iter().any(|s| s.tenant_id == sub.tenant_id && s.uri == sub.uri && s.conn == sub.conn) {
            return Ok(());
        }
        let open = reg.subs.iter().filter(|s| s.tenant_id == sub.tenant_id).count();
        if open as i64 >= max {
            return Err(open);
        }
        reg.subs.push(sub);
        Ok(())
    }

    /// Remember that `conn` listed `tenant_id`'s resources, so it hears
    /// `notifications/resources/list_changed` (requirement 6).
    pub fn watch_list(&self, tenant_id: i64, conn: &str, peer: rmcp::service::Peer<rmcp::RoleServer>) {
        let mut reg = self.lock();
        reg.prune();
        if !reg.watchers.iter().any(|s| s.tenant_id == tenant_id && s.conn == conn) {
            reg.watchers.push(Subscription { tenant_id, uri: String::new(), conn: conn.to_string(), peer });
        }
    }

    /// Remove `(tenant, uri, conn)`; unknown entries are a no-op.
    pub fn remove(&self, tenant_id: i64, uri: &str, conn: &str) {
        self.lock().subs.retain(|s| !(s.tenant_id == tenant_id && s.uri == uri && s.conn == conn));
    }

    /// Send `notifications/resources/updated {uri}` to every live subscriber
    /// of `(tenant_id, uri)`. Best-effort and off the caller's path: the
    /// send runs on its own task, and a peer whose send fails is dropped on
    /// that first error.
    pub fn notify_updated(&self, tenant_id: i64, uri: &str) {
        let targets: Vec<_> = {
            let mut reg = self.lock();
            reg.prune();
            reg.subs
                .iter()
                .filter(|s| s.tenant_id == tenant_id && s.uri == uri)
                .map(|s| (s.conn.clone(), s.peer.clone()))
                .collect()
        };
        if targets.is_empty() {
            return;
        }
        let this = self.clone();
        let uri = uri.to_string();
        tokio::spawn(async move {
            for (conn, peer) in targets {
                if let Err(e) = peer.notify_resource_updated(rmcp::model::ResourceUpdatedNotificationParam::new(uri.clone())).await {
                    tracing::debug!(error = %e, uri = %uri, "dropping a dead resource subscriber");
                    this.remove(tenant_id, &uri, &conn);
                }
            }
        });
    }

    /// Send `notifications/resources/list_changed` to every live connection
    /// of `tenant_id` that listed resources or subscribed to one.
    pub fn notify_list_changed(&self, tenant_id: i64) {
        let targets: Vec<_> = {
            let mut reg = self.lock();
            reg.prune();
            let mut seen: Vec<String> = Vec::new();
            let mut peers = Vec::new();
            for s in reg.watchers.iter().chain(reg.subs.iter()).filter(|s| s.tenant_id == tenant_id) {
                if !seen.contains(&s.conn) {
                    seen.push(s.conn.clone());
                    peers.push(s.peer.clone());
                }
            }
            peers
        };
        if targets.is_empty() {
            return;
        }
        tokio::spawn(async move {
            for peer in targets {
                if let Err(e) = peer.notify_resource_list_changed().await {
                    tracing::debug!(error = %e, "resource list_changed not delivered");
                }
            }
        });
    }
}

/// Why a subscribe was refused.
#[derive(Debug)]
pub enum SubscribeError {
    NotFound(String),
    Forbidden { scope: &'static str },
}

/// Validate `uri` for `tenant` as a subscription target (a known template,
/// and a scope that covers it) and return its template.
pub fn check_subscribable(uri: &str, token_scope: Option<&str>) -> Result<&'static ResourceTemplate, SubscribeError> {
    let parsed = parse_uri(uri).ok_or_else(|| SubscribeError::NotFound(format!("no resource at {uri}")))?;
    let template = template_for(parsed.kind);
    if !crate::oauth::scope_satisfied(token_scope, &[template.scope.to_string()]) {
        return Err(SubscribeError::Forbidden { scope: template.scope });
    }
    Ok(template)
}

/// Register a subscription for `tenant` on `uri` over `conn`'s `peer`,
/// refusing with `resources_max` (naming `data.limit`) once the tenant's
/// plan ceiling of open subscriptions is reached.
pub fn subscribe(
    state: &AppState,
    tenant: &Tenant,
    uri: &str,
    conn: &str,
    peer: rmcp::service::Peer<rmcp::RoleServer>,
) -> Result<(), AppError> {
    let plan = state
        .plans
        .get(&tenant.plan)
        .ok_or_else(|| AppError::Internal(format!("tenant's plan '{}' is not in the loaded plan catalog", tenant.plan)))?;
    let limit = plan.resources_max;
    let sub = Subscription { tenant_id: tenant.id, uri: uri.to_string(), conn: conn.to_string(), peer };
    state.resource_subs.add(sub, limit).map_err(|open| AppError::Structured {
        code: RESOURCES_MAX,
        message: format!("this tenant already has {open} open resource subscriptions, the {} plan maximum", plan.name),
        data: json!({"limit": limit, "used": open, "plan": plan.name}),
    })
}

/// What a write verb tells `resources/*` subscribers once it has committed.
#[derive(Debug, Default)]
pub struct WriteHint {
    /// The resource whose contents changed, as (kind, key).
    updated: Option<(ResourceKind, String)>,
    /// The verb added or removed a resource.
    list_changed: bool,
    /// Take the updated key from the verb's result `id` instead.
    key_from_result_id: bool,
}

/// The hint for `tool` called with `args`, computed before the call so the
/// args need not outlive it; `None` for every verb that is not a resource
/// write.
pub fn write_hint(tool: &str, args: &Value) -> Option<WriteHint> {
    let arg = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_string);
    Some(match tool {
        "host.table.append" => WriteHint { updated: arg("table").map(|t| (ResourceKind::Table, t)), ..Default::default() },
        "host.table.create" => WriteHint { list_changed: true, ..Default::default() },
        "host.table.drop" => WriteHint {
            updated: arg("table").or_else(|| arg("name")).map(|t| (ResourceKind::Table, t)),
            list_changed: true,
            ..Default::default()
        },
        "host.docs.put" | "host.docs.delete" => {
            WriteHint { updated: Some((ResourceKind::Doc, String::new())), list_changed: true, key_from_result_id: true }
        }
        "host.tool.publish" | "host.tool.remove" => {
            WriteHint { updated: arg("name").map(|n| (ResourceKind::Tool, n)), list_changed: true, ..Default::default() }
        }
        _ => return None,
    })
}

/// Tell the tenant's subscribers about a committed write.
pub fn after_write(state: &AppState, tenant_id: i64, hint: WriteHint, out: &Value) {
    if let Some((kind, key)) = hint.updated {
        let key = if hint.key_from_result_id { out.get("id").and_then(Value::as_str).map(str::to_string) } else { Some(key) };
        if let Some(key) = key {
            state.resource_subs.notify_updated(tenant_id, &uri_for(kind, &key));
        }
    }
    if hint.list_changed {
        state.resource_subs.notify_list_changed(tenant_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_section_keys_equal_the_template_kinds() {
        let section = contract_section(RESOURCE_TEMPLATES);
        let kinds: std::collections::BTreeSet<&str> = RESOURCE_TEMPLATES.iter().map(|t| t.kind.key()).collect();
        for field in ["uri_scheme", "name_field"] {
            let keys: std::collections::BTreeSet<&str> =
                section[field].as_object().expect("object").keys().map(String::as_str).collect();
            assert_eq!(keys, kinds, "{field} is keyed by every template kind");
        }
    }

    #[test]
    fn contract_templates_are_what_uri_for_and_list_entry_emit() {
        let section = contract_section(RESOURCE_TEMPLATES);
        for t in RESOURCE_TEMPLATES {
            let key = t.kind.key();
            let ph = format!("{{{}}}", placeholder(t));
            assert_eq!(uri_for(t.kind, &ph), section["uri_scheme"][key].as_str().unwrap());
            let entry = list_entry(t.kind, "x1", None);
            assert_eq!(entry.uri, section["uri_scheme"][key].as_str().unwrap().replace(&ph, "x1"));
            assert_eq!(entry.name, section["name_field"][key].as_str().unwrap().replace(&ph, "x1"));
        }
    }
}

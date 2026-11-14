//! PRD-mcphost-tenant-templates: `host.template_apply <name>` provisions a
//! whole job -- tools, tables, triggers -- in one call.
//!
//! A template is a data file under `templates/` (embedded below, parsed once).
//! Apply reuses the in-process handlers behind `host.tool_publish`,
//! `host.table.create` and `host.trigger.set`, tags every object it creates in
//! `template_objects`, and returns the demo call for the agent to run itself.

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::db::{Tenant, TemplateObject};
use crate::errors::AppError;
use crate::state::AppState;

/// `(name, toml source)` of every shipped template; the drift test checks this
/// list equals the files in `templates/`.
pub const SOURCES: &[(&str, &str)] = &[
    ("scheduled-job", include_str!("../templates/scheduled-job.toml")),
    ("webhook-inbox", include_str!("../templates/webhook-inbox.toml")),
    ("uptime-probes", include_str!("../templates/uptime-probes.toml")),
    ("database-in-a-minute", include_str!("../templates/database-in-a-minute.toml")),
    ("docs-qa", include_str!("../templates/docs-qa.toml")),
];

#[derive(Debug, Clone, Deserialize)]
pub struct Param {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(default)]
    pub required: bool,
    pub default: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub kind: String,
    pub spec: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TableDef {
    pub name: String,
    pub columns: Map<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TriggerDef {
    pub tool: String,
    pub kind: String,
    pub schedule: Option<String>,
    pub verify: Option<String>,
}

/// What a wrapping template's verb creates, for the plan check that runs before
/// the verb is called.
#[derive(Debug, Clone, Deserialize)]
pub struct Creates {
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub tables: Vec<String>,
    #[serde(default)]
    pub triggers: Vec<String>,
}

/// A template that delegates to an existing one-call verb rather than creating
/// its objects itself.
#[derive(Debug, Clone, Deserialize)]
pub struct Wrap {
    pub verb: String,
    pub args: Value,
    pub creates: Creates,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Demo {
    pub call: Value,
    pub expect: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Template {
    #[serde(skip)]
    pub name: String,
    pub title: String,
    pub one_line: String,
    pub plan_fit: String,
    #[serde(default)]
    pub params: Vec<Param>,
    #[serde(default)]
    pub tools: Vec<ToolDef>,
    #[serde(default)]
    pub tables: Vec<TableDef>,
    #[serde(default)]
    pub triggers: Vec<TriggerDef>,
    #[serde(default)]
    pub secrets_required: Vec<String>,
    pub wrap: Option<Wrap>,
    pub demo: Demo,
    #[serde(default)]
    pub next: Vec<Value>,
}

/// Every shipped template, parsed. A file that fails to parse is a build-time
/// bug the drift test names; it is skipped here so the host still starts.
pub fn all() -> &'static [Template] {
    static ALL: LazyLock<Vec<Template>> = LazyLock::new(|| {
        SOURCES
            .iter()
            .filter_map(|(name, src)| {
                let mut t: Template = toml::from_str(src).ok()?;
                t.name = (*name).to_string();
                Some(t)
            })
            .collect()
    });
    &ALL
}

/// Every template name, in file order: the `enum` of `host.template_apply`'s `name`.
pub fn names() -> Vec<&'static str> {
    all().iter().map(|t| t.name.as_str()).collect()
}

pub fn find(name: &str) -> Option<&'static Template> {
    all().iter().find(|t| t.name == name)
}

/// `{{param}}` substitution over every string in `v`.
fn substitute(v: &Value, params: &Map<String, Value>) -> Value {
    match v {
        Value::String(s) => {
            // A string that is exactly one `{{param}}` takes the param's own type
            // (an array of URLs stays an array).
            if let Some(name) = s.strip_prefix("{{").and_then(|r| r.strip_suffix("}}"))
                && let Some(val) = params.get(name.trim())
            {
                return val.clone();
            }
            let mut out = s.clone();
            for (k, val) in params {
                let text = match val {
                    Value::String(x) => x.clone(),
                    other => other.to_string(),
                };
                out = out.replace(&format!("{{{{{k}}}}}"), &text);
            }
            Value::String(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| substitute(x, params)).collect()),
        Value::Object(m) => Value::Object(m.iter().map(|(k, x)| (k.clone(), substitute(x, params))).collect()),
        other => other.clone(),
    }
}

/// Validated params merged over the defaults.
fn resolve_params(t: &Template, args: &Value) -> Result<Map<String, Value>, AppError> {
    let given = match args.get("params") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(m)) => m.clone(),
        Some(_) => return Err(AppError::InvalidArgs("params: must be an object".into())),
    };
    for k in given.keys() {
        if !t.params.iter().any(|p| &p.name == k) {
            let known: Vec<&str> = t.params.iter().map(|p| p.name.as_str()).collect();
            return Err(AppError::InvalidArgs(format!(
                "params.{k}: template '{}' takes {}",
                t.name,
                if known.is_empty() { "no params".to_string() } else { known.join(", ") }
            )));
        }
    }
    let mut out = Map::new();
    for p in &t.params {
        let v = match given.get(&p.name).or(p.default.as_ref()) {
            Some(v) => v.clone(),
            None if p.required => {
                return Err(AppError::InvalidArgs(format!("params.{}: required by template '{}'", p.name, t.name)));
            }
            None => continue,
        };
        let ok = match p.ty.as_str() {
            "integer" => v.is_i64() || v.is_u64(),
            "boolean" => v.is_boolean(),
            "array" => v.is_array(),
            _ => v.is_string(),
        };
        if !ok {
            return Err(AppError::InvalidArgs(format!("params.{}: must be a {}", p.name, p.ty)));
        }
        out.insert(p.name.clone(), v);
    }
    Ok(out)
}

fn tag(t: &Template, kind: &str, name: &str, version: Option<i64>, object_id: Option<String>) -> TemplateObject {
    TemplateObject { template: t.name.clone(), kind: kind.to_string(), name: name.to_string(), version, object_id }
}

crate::err_code! {
    pub CONFLICT,
    code: "conflict", family: Call, class: AgentFault, retryable: true,
    remedy: "Another template call is running for this tenant; wait data.retry_after_s seconds and retry.",
    data_keys: &["retry_after_s"],
}

/// How long a refused caller waits before retrying (the apply it lost to is a
/// handful of in-process creates).
pub const RETRY_AFTER_S: u64 = 2;

fn locked() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static LOCKED: LazyLock<std::sync::Mutex<std::collections::HashSet<String>>> = LazyLock::new(Default::default);
    &LOCKED
}

/// R3: the per-tenant lock `host.template_apply` and `host.template_remove`
/// hold for their whole run. Released on drop.
#[derive(Debug)]
pub struct TenantLock(String);

impl Drop for TenantLock {
    fn drop(&mut self) {
        locked().lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&self.0);
    }
}

/// Takes `namespace`'s lock, or answers `conflict` with `data.retry_after_s`
/// when another call holds it.
pub fn lock_tenant(namespace: &str) -> Result<TenantLock, AppError> {
    let mut held = locked().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if !held.insert(namespace.to_string()) {
        return Err(AppError::Structured {
            code: CONFLICT,
            message: format!("another template call is running for {namespace}; retry in {RETRY_AFTER_S}s"),
            data: json!({"retry_after_s": RETRY_AFTER_S}),
        });
    }
    Ok(TenantLock(namespace.to_string()))
}

/// What a template would create, by ceiling: tool names, table names and the
/// kind of each trigger.
struct Bundle {
    tools: Vec<String>,
    tables: Vec<String>,
    trigger_kinds: Vec<String>,
}

fn bundle(t: &Template, params: &Map<String, Value>) -> Bundle {
    let sub = |s: &str| substitute(&json!(s), params).as_str().unwrap_or_default().to_string();
    match &t.wrap {
        Some(w) => Bundle {
            tools: w.creates.tools.iter().map(|x| sub(x)).collect(),
            tables: w.creates.tables.iter().map(|x| sub(x)).collect(),
            trigger_kinds: w.creates.triggers.clone(),
        },
        None => Bundle {
            tools: t.tools.iter().map(|x| sub(&x.name)).collect(),
            tables: t.tables.iter().map(|x| sub(&x.name)).collect(),
            trigger_kinds: t.triggers.iter().map(|x| x.kind.clone()).collect(),
        },
    }
}

fn plan_limit(ceiling: &str, limit: i64, used: i64, needed: i64) -> AppError {
    AppError::Structured {
        code: crate::handler::PLAN_LIMIT,
        message: format!(
            "this plan allows {limit} {ceiling}; this tenant holds {used} and the template needs {needed} more -- nothing was created"
        ),
        data: json!({"ceiling": ceiling, "limit": limit, "used": used, "needed": needed}),
    }
}

/// R2: every plan ceiling the bundle would touch, checked before any create; the
/// first one it would exceed is named in `data.ceiling`. Objects this template
/// already tagged are not counted again (a re-apply updates them).
async fn check_ceilings(state: &AppState, tenant: &Tenant, t: &Template, b: &Bundle) -> Result<(), AppError> {
    let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
        AppError::Internal(format!("tenant's plan '{}' is not in the loaded plan catalog", tenant.plan))
    })?;
    let tagged = state.db.template_objects(tenant.id, t.name.clone()).await?;
    let tagged_of = |kind: &str| tagged.iter().filter(|o| o.kind == kind).map(|o| o.name.as_str()).collect::<Vec<_>>();

    let tagged_tools = tagged_of("tool");
    let new_tools = b.tools.iter().filter(|n| !tagged_tools.contains(&n.as_str())).count() as i64;
    if new_tools > 0 {
        let used = state.db.count_tools(tenant.id).await?;
        let limit = plan.tools_max.min(crate::state::MAX_TOOLS_PER_TENANT);
        if used + new_tools > limit {
            return Err(plan_limit("tools_max", limit, used, new_tools));
        }
    }

    let tagged_triggers = tagged_of("trigger").len();
    let fresh = |kinds: &[&str]| -> i64 {
        let wanted = b.trigger_kinds.iter().filter(|k| kinds.contains(&k.as_str())).count();
        if tagged_triggers >= b.trigger_kinds.len() { 0 } else { wanted as i64 }
    };
    let new_schedule = fresh(&["schedule", "webhook"]);
    if new_schedule > 0 {
        let used = state.db.count_schedule_triggers_for_tenant(tenant.id).await?;
        if used + new_schedule > plan.schedules_max {
            return Err(plan_limit("schedules_max", plan.schedules_max, used, new_schedule));
        }
    }
    let new_event = fresh(&["event", "message", "webhook"]);
    if new_event > 0 {
        let used = state.db.count_event_triggers_for_tenant(tenant.id).await?
            + state.db.count_webhook_triggers_for_tenant(tenant.id).await?;
        if used + new_event > plan.event_triggers_max {
            return Err(plan_limit("event_triggers_max", plan.event_triggers_max, used, new_event));
        }
    }

    let tagged_tables = tagged_of("table");
    let new_tables = b.tables.iter().filter(|n| !tagged_tables.contains(&n.as_str())).count() as i64;
    if new_tables > 0 {
        let listed = crate::tables::table_list(state, tenant, &json!({})).await?;
        let used = listed["tables"].as_array().map_or(0, Vec::len) as i64;
        if used + new_tables > plan.table_tables_max {
            return Err(plan_limit("table_tables_max", plan.table_tables_max, used, new_tables));
        }
    }
    Ok(())
}

/// `host.template_apply(name, params?)`.
pub async fn apply(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'name'".into()))?;
    let t = find(name).ok_or_else(|| {
        AppError::InvalidArgs(format!(
            "name: no template '{name}'; one of {}",
            all().iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", ")
        ))
    })?;
    apply_template(state, tenant, t, args).await
}

/// Applies `t` (a shipped template, or a fixture a test parsed itself).
pub async fn apply_template(state: &AppState, tenant: &Tenant, t: &Template, args: &Value) -> Result<Value, AppError> {
    let params = resolve_params(t, args)?;
    let _lock = lock_tenant(&tenant.namespace)?;
    // A template that needs a secret the tenant has not set stops before any create
    // and names what to set.
    if !t.secrets_required.is_empty() {
        let have = state.db.list_secret_names(tenant.id).await?;
        let missing: Vec<&String> = t.secrets_required.iter().filter(|s| !have.contains(s)).collect();
        if !missing.is_empty() {
            let none = json!({"tools": [], "tables": [], "triggers": []});
            return Ok(json!({
                "template": t.name,
                "created": none,
                "updated": none,
                "secrets_required": missing,
                "demo": substitute(&serde_json::to_value(&t.demo).unwrap_or(Value::Null), &params),
                "next": [{"tool": "host.secret.set", "note": "set each name in secrets_required, then apply again"}],
            }));
        }
    }
    let b = bundle(t, &params);
    check_ceilings(state, tenant, t, &b).await?;
    refuse_foreign_tools(state, tenant, t, &b).await?;

    let mut made: Vec<TemplateObject> = Vec::new();
    match create_all(state, tenant, t, &params, &mut made).await {
        Ok((created, updated)) => Ok(json!({
            "template": t.name,
            "created": created,
            "updated": updated,
            "demo": substitute(&serde_json::to_value(&t.demo).unwrap_or(Value::Null), &params),
            "next": substitute(&Value::Array(t.next.clone()), &params),
        })),
        Err(e) => {
            rollback(state, tenant, made).await;
            Err(e)
        }
    }
}

/// A tool the tenant already owns under a name the template would publish is
/// never overwritten unless this template made it.
async fn refuse_foreign_tools(state: &AppState, tenant: &Tenant, t: &Template, b: &Bundle) -> Result<(), AppError> {
    let tagged = state.db.template_objects(tenant.id, t.name.clone()).await?;
    for tool in &b.tools {
        let ours = tagged.iter().any(|o| o.kind == "tool" && &o.name == tool);
        if !ours && state.db.get_tool(tenant.id, tool.clone()).await?.is_some() {
            return Err(AppError::InvalidArgs(format!(
                "params.name: the tenant already has a tool '{tool}' that template '{}' did not create; pass another name",
                t.name
            )));
        }
    }
    Ok(())
}

/// Creates tools, then tables, then triggers (a wrapping template calls its one
/// verb), tagging each object. An object this template already made is updated
/// in place (R3) and listed under `updated`; each new one is also pushed to
/// `made`, so a later failure undoes exactly what this call created.
async fn create_all(
    state: &AppState,
    tenant: &Tenant,
    t: &Template,
    params: &Map<String, Value>,
    made: &mut Vec<TemplateObject>,
) -> Result<(Value, Value), AppError> {
    let tagged = state.db.template_objects(tenant.id, t.name.clone()).await?;
    let was_tagged = |kind: &str, name: &str| tagged.iter().any(|o| o.kind == kind && o.name == name);
    let mut created = Objects::default();
    let mut updated = Objects::default();
    let mut url = None;

    if let Some(w) = &t.wrap {
        let mut verb_args = substitute(&w.args, params);
        // An optional param the caller left out leaves its `{{placeholder}}` behind: drop it.
        if let Some(obj) = verb_args.as_object_mut() {
            obj.retain(|_, v| !v.as_str().is_some_and(|s| s.contains("{{")));
        }
        let wrapped = run_wrap(state, tenant, w, &verb_args, &tagged).await?;
        url = wrapped.url;
        let mut keep = |kind: &str, name: String, version: Option<i64>, object_id: Option<String>| {
            let obj = tag(t, kind, &name, version, object_id);
            let list = if wrapped.fresh { &mut created } else { &mut updated };
            match kind {
                "tool" => list.tools.push(name),
                "table" => list.tables.push(name),
                _ => list.triggers.push(name),
            }
            obj
        };
        let mut objs = Vec::new();
        for tool in &wrapped.tools {
            let version = state.db.get_tool(tenant.id, tool.clone()).await?.map(|r| r.current_version);
            objs.push(keep("tool", tool.clone(), version, None));
        }
        for table in &wrapped.tables {
            let rows = table_rows(state, tenant, table).await?;
            objs.push(keep("table", table.clone(), rows, None));
        }
        for (id, tool) in &wrapped.triggers {
            objs.push(keep("trigger", id.clone(), None, Some(tool.clone())));
        }
        for obj in objs {
            if wrapped.fresh {
                record(state, tenant, made, obj).await?;
            } else {
                state.db.template_object_put(tenant.id, obj).await?;
            }
        }
    } else {
        for tool in &t.tools {
            let tool_name = substitute(&json!(tool.name), params).as_str().unwrap_or_default().to_string();
            let spec = substitute(&tool.spec, params);
            let existed = was_tagged("tool", &tool_name) && state.db.get_tool(tenant.id, tool_name.clone()).await?.is_some();
            let out = crate::control::tool_publish(
                state,
                tenant,
                &json!({"name": tool_name, "kind": tool.kind, "spec": spec}),
            )
            .await?;
            let tool_tag = tag(t, "tool", &tool_name, out["version"].as_i64(), None);
            if existed {
                state.db.template_object_put(tenant.id, tool_tag).await?;
                updated.tools.push(tool_name);
            } else {
                record(state, tenant, made, tool_tag).await?;
                created.tools.push(tool_name);
            }
        }
        for table in &t.tables {
            let table_name = substitute(&json!(table.name), params).as_str().unwrap_or_default().to_string();
            if was_tagged("table", &table_name) && crate::tables::table_exists(state, tenant, &table_name).await? {
                updated.tables.push(table_name);
                continue;
            }
            crate::tables::table_create(state, tenant, &json!({"name": table_name, "columns": table.columns})).await?;
            let rows = table_rows(state, tenant, &table_name).await?;
            record(state, tenant, made, tag(t, "table", &table_name, rows, None)).await?;
            created.tables.push(table_name);
        }
        for trig in &t.triggers {
            let mut set = json!({"tool": substitute(&json!(trig.tool), params), "kind": trig.kind});
            if let Some(s) = &trig.schedule {
                set["schedule"] = json!(s);
            }
            if let Some(v) = &trig.verify {
                set["verify"] = json!(v);
            }
            let out = crate::triggers::set(state, tenant, &set).await?;
            let id = out.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
            let tool_name = set["tool"].as_str().unwrap_or_default().to_string();
            let trig_tag = tag(t, "trigger", &id, None, Some(tool_name));
            if out.get("created").and_then(Value::as_bool).unwrap_or(true) {
                record(state, tenant, made, trig_tag).await?;
                created.triggers.push(id);
            } else {
                state.db.template_object_put(tenant.id, trig_tag).await?;
                updated.triggers.push(id);
            }
        }
    }

    let mut created = created.json();
    if let Some(url) = url {
        created["url"] = url;
    }
    Ok((created, updated.json()))
}

/// What a wrapped verb left behind: the objects to tag, and whether this call
/// created them (`fresh`) or found them already there.
struct Wrapped {
    url: Option<Value>,
    fresh: bool,
    tools: Vec<String>,
    tables: Vec<String>,
    /// `(trigger id, the tool it fires)`.
    triggers: Vec<(String, String)>,
}

/// Calls the verb `w` wraps, in process, and reports what it made.
async fn run_wrap(
    state: &AppState,
    tenant: &Tenant,
    w: &Wrap,
    verb_args: &Value,
    tagged: &[TemplateObject],
) -> Result<Wrapped, AppError> {
    match w.verb.as_str() {
        "host.webhook_inbox.create" => {
            let out = crate::webhook_inbox::create(state, tenant, verb_args).await?;
            let tool = out.get("tool").and_then(Value::as_str).unwrap_or_default().to_string();
            Ok(Wrapped {
                url: out.get("url").cloned(),
                fresh: out.get("created").and_then(Value::as_bool).unwrap_or(true),
                triggers: out
                    .get("trigger_id")
                    .and_then(Value::as_str)
                    .map(|id| vec![(id.to_string(), tool.clone())])
                    .unwrap_or_default(),
                tools: vec![tool],
                tables: Vec::new(),
            })
        }
        "host.uptime.create" => {
            let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
                AppError::Internal(format!("tenant's plan '{}' is not in the loaded plan catalog", tenant.plan))
            })?;
            let create = crate::uptime::parse_create(&state.kinds, plan, verb_args)?;
            // A re-apply rebuilds the probe set in place: what this template made
            // earlier is removed first, so the same names can be created again.
            let previous: Vec<TemplateObject> = tagged.to_vec();
            let fresh = previous.is_empty();
            if !fresh {
                rollback(state, tenant, previous).await;
            }
            let trigger = crate::uptime::create_assets(state, tenant, &create).await?;
            let id = trigger.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
            let probe = crate::uptime::probe_tool_name(&create.name);
            Ok(Wrapped {
                url: None,
                fresh,
                tools: vec![create.name.clone(), probe.clone()],
                tables: vec![crate::uptime::targets_table(&create.name)],
                triggers: vec![(id, probe)],
            })
        }
        other => Err(AppError::Internal(format!("template wraps unsupported verb {other}"))),
    }
}

/// Object names by kind, as `created` / `updated` list them.
#[derive(Default)]
struct Objects {
    tools: Vec<String>,
    tables: Vec<String>,
    triggers: Vec<String>,
}

impl Objects {
    fn json(&self) -> Value {
        json!({"tools": self.tools, "tables": self.tables, "triggers": self.triggers})
    }
}

/// The row count of `table`, `None` when the tenant has no such table.
async fn table_rows(state: &AppState, tenant: &Tenant, table: &str) -> Result<Option<i64>, AppError> {
    let listed = crate::tables::table_list(state, tenant, &json!({})).await?;
    Ok(listed["tables"]
        .as_array()
        .and_then(|all| all.iter().find(|t| t["name"] == json!(table)))
        .and_then(|t| t["rows"].as_i64()))
}

/// Tags `obj` in the database and remembers it for rollback.
async fn record(state: &AppState, tenant: &Tenant, made: &mut Vec<TemplateObject>, obj: TemplateObject) -> Result<(), AppError> {
    made.push(obj.clone());
    state.db.template_object_put(tenant.id, obj).await
}

/// R2's all-or-nothing: removes what this call created, newest first (triggers,
/// then tables, then tools), through each object's own remove path, and drops
/// its tag. Best effort -- the create's error is what the caller sees.
async fn rollback(state: &AppState, tenant: &Tenant, made: Vec<TemplateObject>) {
    for obj in made.into_iter().rev() {
        let removed = match obj.kind.as_str() {
            "trigger" => state.db.remove_trigger(tenant.id, obj.name.clone()).await.map(|_| ()),
            "table" => crate::tables::table_drop(state, tenant, &json!({"name": obj.name, "confirm": true})).await.map(|_| ()),
            _ => state.db.remove_tool(tenant.id, obj.name.clone()).await.map(|_| ()),
        };
        if let Err(e) = removed {
            tracing::warn!(error = %e, template = %obj.template, kind = %obj.kind, name = %obj.name, "template rollback could not remove an object");
        }
        if let Err(e) = state.db.template_object_delete(tenant.id, obj).await {
            tracing::warn!(error = %e, "template rollback could not drop a tag");
        }
    }
}

/// How many objects of each kind `t` creates, from its default params.
fn creates_counts(t: &Template) -> Value {
    let defaults: Map<String, Value> = t.params.iter().filter_map(|p| p.default.clone().map(|d| (p.name.clone(), d))).collect();
    let b = bundle(t, &defaults);
    json!({"tools": b.tools.len(), "tables": b.tables.len(), "triggers": b.trigger_kinds.len()})
}

/// One template as `host.template_list` and `GET /templates.json` describe it.
fn summary(t: &Template) -> Value {
    json!({
        "name": t.name,
        "title": t.title,
        "one_line": t.one_line,
        "plan_fit": t.plan_fit,
        "creates": creates_counts(t),
        "params": t.params.iter().map(|p| json!({"name": p.name, "type": p.ty, "required": p.required, "default": p.default})).collect::<Vec<_>>(),
        "secrets_required": t.secrets_required,
    })
}

/// R4: the one listing `host.template_list` (anonymous and keyed) and
/// `GET /templates.json` all return.
pub fn list_json() -> Value {
    json!({"templates": all().iter().map(summary).collect::<Vec<_>>(), "count": all().len()})
}

/// R4: the compact form `host.quickstart` carries as `templates`.
pub fn quickstart_json() -> Value {
    json!(all()
        .iter()
        .map(|t| json!({"name": t.name, "one_line": t.one_line, "plan_fit": t.plan_fit, "creates": creates_counts(t)}))
        .collect::<Vec<_>>())
}

/// The trigger kinds a template may set (the real ones, never job-word aliases).
pub const TRIGGER_KINDS: &[&str] = &["schedule", "event", "message", "webhook"];

/// Every `{{param}}` reference in `text`.
fn param_refs(text: &str) -> Vec<&str> {
    text.split("{{").skip(1).filter_map(|rest| rest.split_once("}}").map(|(name, _)| name.trim())).collect()
}

/// R5: what is wrong with one template file, each line naming the file and the
/// field. `verbs` is every name `host_tools` serves (aliases included); `kinds`
/// is the registry the host runs. An empty list means the file resolves.
pub fn drift(file: &str, source: &str, kinds: &crate::kinds::KindRegistry, verbs: &[String]) -> Vec<String> {
    let t: Template = match toml::from_str(source) {
        Ok(t) => t,
        Err(e) => return vec![format!("{file}: does not parse: {}", e.message())],
    };
    let mut out = Vec::new();
    let kind_names = kinds.names();
    let aliases = crate::kinds::aliases::alias_names();
    for (i, tool) in t.tools.iter().enumerate() {
        if aliases.contains(&tool.kind.as_str()) {
            out.push(format!("{file}: tools[{i}].kind: '{}' is a job-word alias, not a registered kind", tool.kind));
            continue;
        }
        if !kind_names.contains(&tool.kind.as_str()) {
            out.push(format!("{file}: tools[{i}].kind: '{}' is not in KindRegistry::names() ({})", tool.kind, kind_names.join(", ")));
            continue;
        }
        let known = kinds.get(&tool.kind).map(|k| k.known_spec_fields()).unwrap_or(&[]);
        if let Some(spec) = tool.spec.as_object() {
            for field in spec.keys().filter(|f| !known.is_empty() && !known.contains(&f.as_str())) {
                out.push(format!("{file}: tools[{i}].spec.{field}: not a spec field of kind '{}' ({})", tool.kind, known.join(", ")));
            }
        }
    }
    if let Some(w) = &t.wrap {
        if !verbs.contains(&w.verb) {
            out.push(format!("{file}: wrap.verb: '{}' is not in host_tools", w.verb));
        }
        for (i, kind) in w.creates.triggers.iter().enumerate() {
            if !TRIGGER_KINDS.contains(&kind.as_str()) {
                out.push(format!("{file}: wrap.creates.triggers[{i}]: '{kind}' is not a trigger kind ({})", TRIGGER_KINDS.join(", ")));
            }
        }
    }
    let tool_names: Vec<&str> = t.tools.iter().map(|x| x.name.as_str()).collect();
    for (i, trig) in t.triggers.iter().enumerate() {
        if !TRIGGER_KINDS.contains(&trig.kind.as_str()) {
            out.push(format!("{file}: triggers[{i}].kind: '{}' is not a trigger kind ({})", trig.kind, TRIGGER_KINDS.join(", ")));
        }
        if trig.kind == "schedule" && trig.schedule.is_none() {
            out.push(format!("{file}: triggers[{i}].schedule: a schedule trigger needs one"));
        }
        if !tool_names.contains(&trig.tool.as_str()) {
            out.push(format!("{file}: triggers[{i}].tool: '{}' is not one of the template's tools[]", trig.tool));
        }
    }
    let demo_tool = t.demo.call.get("tool").and_then(Value::as_str).unwrap_or_default();
    if !verbs.iter().any(|v| v == demo_tool) {
        out.push(format!("{file}: demo.call.tool: '{demo_tool}' is not in host_tools"));
    }
    if t.demo.expect.as_object().is_none_or(Map::is_empty) {
        out.push(format!("{file}: demo.expect: must be a non-empty object"));
    }
    for (i, next) in t.next.iter().enumerate() {
        let verb = next.get("tool").and_then(Value::as_str).unwrap_or_default();
        if !verbs.iter().any(|v| v == verb) {
            out.push(format!("{file}: next[{i}].tool: '{verb}' is not in host_tools"));
        }
    }
    let declared: Vec<&str> = t.params.iter().map(|p| p.name.as_str()).collect();
    for r in param_refs(source) {
        if !declared.contains(&r) {
            out.push(format!("{file}: {{{{{r}}}}}: not a declared param ({})", declared.join(", ")));
        }
    }
    out
}

/// `host.template_remove(name)` (R6): removes what `name` applied that is still
/// tagged and unmodified since -- a tool republished since (its version moved), a
/// table that gained or lost rows, a trigger now firing another tool -- and keeps
/// the rest, naming it under `kept`. Every tag is dropped: a kept object is the
/// user's now.
pub async fn remove(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'name'".into()))?
        .to_string();
    let _lock = lock_tenant(&tenant.namespace)?;
    let tagged = state.db.template_objects(tenant.id, name.clone()).await?;
    let mut removed = Objects::default();
    let mut kept = Objects::default();

    for obj in tagged.iter().filter(|o| o.kind == "trigger") {
        match state.db.get_trigger(tenant.id, obj.name.clone()).await? {
            None => {}
            Some(row) if obj.object_id.as_deref().is_some_and(|tool| tool != row.tool_name) => kept.triggers.push(obj.name.clone()),
            Some(_) => {
                state.db.remove_trigger(tenant.id, obj.name.clone()).await?;
                removed.triggers.push(obj.name.clone());
            }
        }
    }
    for obj in tagged.iter().filter(|o| o.kind == "table") {
        match table_rows(state, tenant, &obj.name).await? {
            None => {}
            Some(rows) if obj.version.is_some_and(|at_apply| at_apply != rows) => kept.tables.push(obj.name.clone()),
            Some(_) => {
                crate::tables::table_drop(state, tenant, &json!({"name": obj.name, "confirm": true})).await?;
                removed.tables.push(obj.name.clone());
            }
        }
    }
    for obj in tagged.iter().filter(|o| o.kind == "tool") {
        match state.db.get_tool(tenant.id, obj.name.clone()).await? {
            None => {}
            Some(row) if obj.version.is_some_and(|at_apply| at_apply != row.current_version) => kept.tools.push(obj.name.clone()),
            Some(_) => {
                state.db.remove_tool(tenant.id, obj.name.clone()).await?;
                removed.tools.push(obj.name.clone());
            }
        }
    }
    for obj in tagged {
        state.db.template_object_delete(tenant.id, obj).await?;
    }
    Ok(json!({"template": name, "removed": removed.json(), "kept": kept.json()}))
}

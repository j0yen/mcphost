//! PRD-mcphost-lineage-blast-radius: "what breaks if this table changes,
//! before it changes". Ports ai-stack's `LineageGraph`/effect table/
//! `blast_radius`/`trace` (`~/projects/ai-stack @ 4ef6c22`) with node kinds
//! rewritten for mcphost's own entities, persisted per tenant in the main
//! `mcphost.db` (migration 0058), and gates `host.table.drop`.
//!
//! - [`graph`]: the ported types (`LineageGraph`, `NodeKind`, `ChangeKind`,
//!   `ImpactReport`, ...).
//! - [`effects`]: the effect table (requirement 2).
//! - [`scan`]: source scanning for `python` tool publishes (requirement 4).
//!
//! This module owns the `host.lineage.*` tools' business logic (pure
//! `AppState` + arguments in, `Value`/`AppError` out, same convention as
//! `control.rs`/`tables_model.rs`) plus the registration/gate entry points
//! `control::tool_publish` and `tables::table_drop` call into.

pub mod effects;
pub mod graph;
pub mod scan;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::db::Tenant;
use crate::errors::AppError;
use crate::state::{AppState, now_unix};

pub use graph::{ChangeKind, ImpactReport, ImpactedNode, LineageGraph, LineageNode, NodeKind, Severity, TraceEntry, node_id};

/// requirement 3: "cached for 30 s".
const CACHE_TTL_SECS: i64 = 30;

/// requirement 8 (AC7): `trace`'s own paged-result window.
const TRACE_PAGE_SIZE: usize = 50;

/// requirement 8 (AC7): a downstream list longer than this is paginated
/// behind a handle instead of returned inline.
const TRACE_INLINE_LIMIT: usize = 100;

/// requirement 6: `blast_radius`'s default `top_n` when the caller omits it.
const DEFAULT_TOP_N: usize = 50;

pub struct CachedGraph {
    graph: Arc<LineageGraph>,
    loaded_at: i64,
}

/// requirement 3: per-tenant, 30s-TTL in-memory cache of the loaded graph --
/// same `Arc<Mutex<HashMap<i64, _>>>` shape as [`crate::billing::AcceptedUsageCache`].
pub type LineageCache = Arc<Mutex<HashMap<i64, CachedGraph>>>;

pub fn new_cache() -> LineageCache {
    Arc::new(Mutex::new(HashMap::new()))
}

/// requirement 8 (AC7): a stored, pageable trace result. Scoped by
/// `tenant_id` so a handle minted for one tenant can never be read back by
/// another (AC8's isolation extends to this store too).
pub struct StoredTracePage {
    tenant_id: i64,
    entries: Vec<TraceEntry>,
}

pub type TracePageCache = Arc<Mutex<HashMap<String, StoredTracePage>>>;

pub fn new_trace_page_cache() -> TracePageCache {
    Arc::new(Mutex::new(HashMap::new()))
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// requirement 8 (AC7): mints an unguessable handle id for a paged trace
/// result -- same `rand::thread_rng().fill_bytes` convention
/// [`crate::session_bind::SessionBindings::issue`] uses.
fn new_trace_handle() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    to_hex(&bytes)
}

fn arg_str(args: &Value, name: &str) -> Result<String, AppError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{name}'")))
}

fn arg_i64_opt(args: &Value, name: &str) -> Option<i64> {
    args.get(name).and_then(Value::as_i64)
}

/// requirement 3: loads (or returns the cached, still-fresh) graph for a
/// tenant.
pub async fn load_graph(state: &AppState, tenant_id: i64) -> Result<Arc<LineageGraph>, AppError> {
    let now = now_unix();
    {
        let cache = state.lineage_cache.lock().unwrap();
        if let Some(cached) = cache.get(&tenant_id)
            && now - cached.loaded_at < CACHE_TTL_SECS
        {
            return Ok(cached.graph.clone());
        }
    }
    let node_rows = state.db.lineage_load_nodes(tenant_id).await?;
    let edge_rows = state.db.lineage_load_edges(tenant_id).await?;
    let mut graph = LineageGraph::new();
    for row in node_rows {
        let Some(kind) = NodeKind::parse(&row.kind) else {
            continue;
        };
        graph.upsert_node(LineageNode {
            id: row.node_id,
            kind,
            label: row.label,
            uses: row.uses.max(0) as u64,
            created_unix: row.created_unix,
            last_used_unix: row.last_used_unix,
            orphaned: row.orphaned,
        });
    }
    for row in edge_rows {
        graph.add_edge(&row.upstream_id, &row.downstream_id, &row.evidence);
    }
    let graph = Arc::new(graph);
    let mut cache = state.lineage_cache.lock().unwrap();
    cache.insert(tenant_id, CachedGraph { graph: graph.clone(), loaded_at: now });
    Ok(graph)
}

/// requirement 3: "invalidated on any edge write for that tenant".
pub fn invalidate_cache(state: &AppState, tenant_id: i64) {
    let mut cache = state.lineage_cache.lock().unwrap();
    cache.remove(&tenant_id);
}

/// requirement 4: registers one edge, creating both endpoint nodes (if they
/// don't already exist) with `uses: 0`. `upstream`/`downstream` are each
/// `(kind, name, label)`. This is the one write path every registration
/// trigger (tool publish's source scan, a chain publish, a chart/handle/
/// document registration) goes through.
pub async fn register_edge(
    state: &AppState,
    tenant_id: i64,
    upstream: (NodeKind, &str, &str),
    downstream: (NodeKind, &str, &str),
    evidence: &str,
) -> Result<(), AppError> {
    let now = now_unix();
    let (up_kind, up_name, up_label) = upstream;
    let (down_kind, down_name, down_label) = downstream;
    let upstream_id = node_id(up_kind, up_name);
    let downstream_id = node_id(down_kind, down_name);
    state.db.lineage_upsert_node(tenant_id, upstream_id.clone(), up_kind.as_str().to_string(), up_label.to_string(), now).await?;
    state
        .db
        .lineage_upsert_node(tenant_id, downstream_id.clone(), down_kind.as_str().to_string(), down_label.to_string(), now)
        .await?;
    state.db.lineage_insert_edge(tenant_id, upstream_id, downstream_id, evidence.to_string(), now).await?;
    invalidate_cache(state, tenant_id);
    Ok(())
}

/// requirement 4 (AC1): scans a `python` tool's source for
/// `mcphost.table.*` calls (evidence `source_scan`) and reads the spec's
/// optional `reads: [table]` declaration (evidence `declared_reads`,
/// requirement 4 -- the only signal a `wasm` tool has). Registers
/// `table:<name> -> tool:<tool_name>` for every table found either way.
/// Best-effort: called after the tool is already stored, never blocks or
/// fails the publish.
pub async fn register_tool_publish(
    state: &AppState,
    tenant_id: i64,
    tool_name: &str,
    kind_name: &str,
    spec: &Value,
) -> Result<(), AppError> {
    let mut tables: Vec<(String, &'static str)> = Vec::new();
    if kind_name == "python"
        && let Some(source) = spec.get("source").and_then(Value::as_str)
    {
        for table in scan::scan_python_source_for_tables(source) {
            tables.push((table, "source_scan"));
        }
    }
    if let Some(reads) = spec.get("reads").and_then(Value::as_array) {
        for table in reads.iter().filter_map(Value::as_str) {
            tables.push((table.to_string(), "declared_reads"));
        }
    }
    for (table, evidence) in tables {
        register_edge(
            state,
            tenant_id,
            (NodeKind::Table, &table, &table),
            (NodeKind::Tool, tool_name, tool_name),
            evidence,
        )
        .await?;
    }
    Ok(())
}

/// requirement 4: "a chain is published (chain → each step's tool)" --
/// registers `tool:<step_tool> -> chain:<chain_name>` for every step, so a
/// table a step's tool reads is transitively reachable from the chain in
/// `blast_radius`/`trace` (AC3/AC6).
pub async fn register_chain_publish(state: &AppState, tenant_id: i64, chain_name: &str, spec: &Value) -> Result<(), AppError> {
    let Some(steps) = spec.get("steps").and_then(Value::as_array) else {
        return Ok(());
    };
    for step in steps {
        let Some(tool) = step.get("tool").and_then(Value::as_str) else {
            continue;
        };
        register_edge(
            state,
            tenant_id,
            (NodeKind::Tool, tool, tool),
            (NodeKind::Chain, chain_name, chain_name),
            "chain_publish",
        )
        .await?;
    }
    Ok(())
}

/// requirement 5: bumps `uses`/`last_used_unix` on a tool or chain node
/// when a run against it finishes, and transitively on every table
/// upstream of it (through registered edges) -- so a table's own `uses`
/// reflects every run of every tool/chain that reads it (AC3's "40 finished
/// runs" reaching `table:orders`'s downstream chain node's own `uses`).
pub async fn record_run_finished(state: &AppState, tenant_id: i64, tool_name: &str) -> Result<(), AppError> {
    let now = now_unix();
    // The node itself may be a `tool` or a `chain` -- try both ids; at most
    // one will exist (a name is one or the other, never both).
    for kind in [NodeKind::Tool, NodeKind::Chain] {
        let id = node_id(kind, tool_name);
        state.db.lineage_bump_uses(tenant_id, id, now).await?;
    }
    invalidate_cache(state, tenant_id);
    // Transitive bump: every table upstream of the tool/chain node, through
    // whatever kind actually has a node (usually exactly one of the two).
    let graph = load_graph(state, tenant_id).await?;
    for kind in [NodeKind::Tool, NodeKind::Chain] {
        let id = node_id(kind, tool_name);
        let (upstream, _downstream) = graph.trace(&id);
        for entry in upstream.into_iter().filter(|e| e.kind == NodeKind::Table) {
            state.db.lineage_bump_uses(tenant_id, entry.id, now).await?;
        }
    }
    invalidate_cache(state, tenant_id);
    Ok(())
}

/// requirement 7: the breaking consumers a `Drop` of `table_name` would
/// hit, ranked the same way `blast_radius` ranks them. Empty when nothing
/// breaking depends on the table.
pub async fn breaking_drop_consumers(state: &AppState, tenant_id: i64, table_name: &str) -> Result<Vec<ImpactedNode>, AppError> {
    let graph = load_graph(state, tenant_id).await?;
    let id = node_id(NodeKind::Table, table_name);
    let report = graph.blast_radius(&id, ChangeKind::Drop, usize::MAX, usize::MAX);
    Ok(report.impacted.into_iter().filter(|n| n.severity == Severity::Breaking).collect())
}

/// requirement 7 cascade: removes `table_name`'s own lineage node and every
/// edge touching it, and flags any dependent `chart`/`handle` node
/// `orphaned` (open question default: "kept, flagged", not deleted).
pub async fn cascade_table_drop(state: &AppState, tenant_id: i64, table_name: &str) -> Result<(), AppError> {
    let graph = load_graph(state, tenant_id).await?;
    let table_id = node_id(NodeKind::Table, table_name);
    let (_upstream, downstream) = graph.trace(&table_id);
    let orphan_ids: Vec<String> = downstream
        .iter()
        .filter(|e| matches!(e.kind, NodeKind::Chart | NodeKind::Handle))
        .map(|e| e.id.clone())
        .collect();
    state.db.lineage_delete_node_and_edges(tenant_id, table_id).await?;
    if !orphan_ids.is_empty() {
        state.db.lineage_mark_orphaned(tenant_id, orphan_ids).await?;
    }
    invalidate_cache(state, tenant_id);
    Ok(())
}

/// requirement 9 (AC9): the non-`none` consumers a `TypeChange` on
/// `table_name` would touch -- `host.table.model_set` reports these, it
/// never blocks on them (unlike the drop gate).
pub async fn change_notes(state: &AppState, tenant_id: i64, table_name: &str, change_kind: ChangeKind) -> Result<Vec<ImpactedNode>, AppError> {
    let graph = load_graph(state, tenant_id).await?;
    let id = node_id(NodeKind::Table, table_name);
    let report = graph.blast_radius(&id, change_kind, usize::MAX, usize::MAX);
    Ok(report.impacted.into_iter().filter(|n| n.severity != Severity::None).collect())
}

fn impacted_to_json(n: &ImpactedNode) -> Value {
    json!({
        "id": n.id,
        "kind": n.kind.as_str(),
        "label": n.label,
        "depth": n.depth,
        "effect": n.effect,
        "severity": match n.severity { Severity::None => "none", Severity::Degrading => "degrading", Severity::Breaking => "breaking" },
        "action": n.action,
        "uses": n.uses,
        "cross_domain": n.cross_domain,
    })
}

/// requirement 6 (AC3/AC10): `host.lineage.blast_radius(id, change_kind,
/// max_depth?, top_n?)`.
pub async fn blast_radius(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let id = arg_str(args, "id")?;
    let change_kind_str = arg_str(args, "change_kind")?;
    let change_kind = ChangeKind::parse(&change_kind_str)
        .ok_or_else(|| AppError::InvalidArgs(format!("change_kind: unrecognized value '{change_kind_str}'")))?;
    let max_depth = arg_i64_opt(args, "max_depth").map(|d| d.max(0) as usize).unwrap_or(usize::MAX);
    let top_n = arg_i64_opt(args, "top_n").map(|n| n.max(0) as usize).unwrap_or(DEFAULT_TOP_N);

    let graph = load_graph(state, tenant.id).await?;
    let report = graph.blast_radius(&id, change_kind, max_depth, top_n);
    Ok(json!({
        "impacted": report.impacted.iter().map(impacted_to_json).collect::<Vec<_>>(),
        "truncated": report.truncated,
    }))
}

fn trace_entry_json(e: &TraceEntry) -> Value {
    json!({"id": e.id, "kind": e.kind.as_str(), "label": e.label, "depth": e.depth, "evidence": e.evidence})
}

/// requirement 8 (AC6/AC7/AC8): `host.lineage.trace(id)`. A downstream list
/// over [`TRACE_INLINE_LIMIT`] is stored behind a handle instead of
/// returned inline (AC7); `upstream` is always inline.
pub async fn trace(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let id = arg_str(args, "id")?;
    let graph = load_graph(state, tenant.id).await?;
    let (upstream, downstream) = graph.trace(&id);

    let mut out = serde_json::Map::new();
    out.insert("id".to_string(), json!(id));
    // requirement 7 cascade (AC4): a traced node's own `orphaned` flag --
    // set when a confirmed table drop cascaded past a dependent chart/
    // handle node. `None` (the id isn't in this tenant's graph at all) is
    // not an error here -- trace answers "what's reachable", not "does
    // this exist".
    if let Some(node) = graph.nodes.get(&id) {
        out.insert("kind".to_string(), json!(node.kind.as_str()));
        out.insert("label".to_string(), json!(node.label));
        out.insert("uses".to_string(), json!(node.uses));
        out.insert("orphaned".to_string(), json!(node.orphaned));
    }
    out.insert("upstream".to_string(), json!(upstream.iter().map(trace_entry_json).collect::<Vec<_>>()));
    out.insert("downstream_count".to_string(), json!(downstream.len()));
    if downstream.len() > TRACE_INLINE_LIMIT {
        let handle = new_trace_handle();
        {
            let mut pages = state.lineage_trace_pages.lock().unwrap();
            pages.insert(handle.clone(), StoredTracePage { tenant_id: tenant.id, entries: downstream });
        }
        out.insert("downstream_handle".to_string(), json!(handle));
    } else {
        out.insert("downstream".to_string(), json!(downstream.iter().map(trace_entry_json).collect::<Vec<_>>()));
    }
    Ok(Value::Object(out))
}

/// requirement 8 (AC7): `host.lineage.trace_page(handle, offset)` -- pages
/// a stored downstream list [`TRACE_PAGE_SIZE`] at a time. Scoped to the
/// calling tenant: a handle minted for another tenant reads back as
/// `lineage_handle_not_found`, same as an unknown one (AC8).
pub async fn trace_page(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let handle = arg_str(args, "handle")?;
    let offset = arg_i64_opt(args, "offset").unwrap_or(0).max(0) as usize;
    let pages = state.lineage_trace_pages.lock().unwrap();
    let Some(stored) = pages.get(&handle).filter(|p| p.tenant_id == tenant.id) else {
        return Err(AppError::Structured {
            code: "lineage_handle_not_found",
            message: "no stored trace result for this handle".to_string(),
            data: Value::Null,
        });
    };
    let total = stored.entries.len();
    let end = (offset + TRACE_PAGE_SIZE).min(total);
    let page: Vec<Value> = if offset < total {
        stored.entries[offset..end].iter().map(trace_entry_json).collect()
    } else {
        Vec::new()
    };
    let next_offset = if end < total { Some(end) } else { None };
    Ok(json!({"entries": page, "offset": offset, "total": total, "next_offset": next_offset}))
}

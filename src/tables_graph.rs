//! PRD-mcphost-table-concept-graph: the tenant's own `table_models` (built
//! by `tables_model.rs`) joined up into one small property graph -- nodes
//! for tables and columns, edges for `column_of`/`foreign_key`/`same_name`
//! -- so an agent can ask "what joins to what" and "what should I ask
//! next" instead of calling `host.table.describe` on every table and
//! inferring the graph itself (Problem statement).
//!
//! Loosely modeled on ai-stack's `mcp-concept-graph` (hand-rolled,
//! serde-only node/edge graph with k-hop and shortest-path traversal) and
//! `mqo-graph-traversal::suggest_next_questions` (Grounding: parts from
//! `~/projects/ai-stack` @ 4ef6c22) -- reimplemented here rather than
//! vendored, since this graph's node/edge kinds (table-shaped: roles,
//! foreign keys, same-name columns) are specific to `tables_model.rs`'s
//! own output, not a generic property graph (Technical considerations).
//!
//! One graph per tenant (`table_graphs`, migration 0058), built from every
//! table this tenant has a stored `table_models` row for -- a table never
//! `describe`d yet simply has no node, the same "every table with a
//! computed model" scoping `tables_model::table_models_list` already uses.
//! Rebuilt by [`tick_once`], called from the tail of
//! [`crate::tables_model::tick_once`] so both stay within the same 30s
//! window `describe` promises (requirement 2).

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::db::Tenant;
use crate::errors::AppError;
use crate::state::AppState;
use crate::tables;

/// requirement 3: `host.table.graph`'s own `schema` field, so a caller can
/// tell this shape apart from some future `table-graph.v2`.
const GRAPH_SCHEMA: &str = "table-graph.v1";

/// requirement 3: `host.table.graph(table?, hops?)`'s `hops` bound.
const MAX_HOPS: i64 = 3;

/// requirement 4: `host.table.join_paths` returns at most this many paths.
const MAX_JOIN_PATHS: usize = 3;

/// A generous bound on how many tables a single join path may cross before
/// giving up on that branch -- large enough that no fixture this PRD's ACs
/// describe (at most a handful of tables) is ever truncated, small enough
/// that a pathological all-to-all `same_name` tenant graph can't make
/// [`find_paths`]' DFS explore an unbounded number of branches.
const MAX_PATH_HOPS: usize = 6;

/// requirement 5: `host.table.next_questions` returns at most this many
/// questions (one per template).
const MAX_NEXT_QUESTIONS: usize = 5;

/// requirement 1: one node -- a table (`kind: "table"`) or a column
/// (`kind` is that column's model role: `key`/`id`/`measure`/`category`/
/// `date`/`text`). `attributes` carries a column node's `table`/`column`/
/// `type`, plus `description` when a `host.table.model_set` annotation set
/// one (requirement 1: "the annotation text is a node attribute, not a
/// node" -- there is deliberately no separate node or edge for it).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    pub kind: String,
    pub attributes: Map<String, Value>,
}

/// requirement 1: one edge. `kind` is `column_of` (column -> its table,
/// weight 1.0, evidence `"declared"`), `foreign_key` (column -> the
/// referenced column, weight 1.0, evidence `"detected"` -- AC1's own
/// wording) or `same_name` (column <-> column of equal name and type in a
/// different table, weight 0.5, evidence `"same_name"`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    pub kind: String,
    pub weight: f64,
    pub evidence: String,
}

/// One tenant's whole graph -- [`build_graph_from_models`]'s return shape,
/// serialized as-is into `table_graphs.graph_json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConceptGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

fn node_attr_str<'a>(node: &'a GraphNode, key: &str) -> &'a str {
    node.attributes.get(key).and_then(Value::as_str).unwrap_or("")
}

/// (column name, declared type) -> every (table, column) sharing both --
/// [`build_graph_from_models`]'s own grouping for requirement 1's
/// `same_name` rule.
type SameNameGroups = HashMap<(String, String), Vec<(String, String)>>;

/// requirement 1: builds one tenant's [`ConceptGraph`] from its own
/// `table_models` rows -- `entries` is `(table_name, annotation-merged
/// model json)` pairs, the same shape [`crate::tables_model::
/// table_describe`] produces per table via `merge_annotations` (so a
/// `role`/`description` override already wins here exactly as it does in
/// `describe`'s own output). A pure function over already-computed models
/// (no SQL, no I/O) so AC10's 50-table/20-column rebuild bound is just
/// this function's own wall-clock cost.
pub fn build_graph_from_models(entries: &[(String, Value)]) -> ConceptGraph {
    let mut nodes: Vec<GraphNode> = Vec::new();
    let mut edges: Vec<GraphEdge> = Vec::new();
    let mut node_ids: HashSet<String> = HashSet::new();
    // requirement 1's `same_name` rule: grouped by (column name, declared
    // type) across every table, so two columns land in the same bucket
    // only when both match -- O(total columns) rather than an O(n^2) scan
    // over every column pair (AC10's perf bound).
    let mut same_name_groups: SameNameGroups = HashMap::new();

    for (table, _model) in entries {
        nodes.push(GraphNode {
            id: table.clone(),
            kind: "table".to_string(),
            attributes: Map::new(),
        });
        node_ids.insert(table.clone());
    }

    for (table, model) in entries {
        let Some(columns) = model.get("columns").and_then(Value::as_object) else {
            continue;
        };
        for (col_name, col_val) in columns {
            let role = col_val.get("role").and_then(Value::as_str).unwrap_or("text").to_string();
            let col_type = col_val.get("type").and_then(Value::as_str).unwrap_or("unknown").to_string();
            let node_id = format!("{table}.{col_name}");

            let mut attributes = Map::new();
            attributes.insert("table".to_string(), json!(table));
            attributes.insert("column".to_string(), json!(col_name));
            attributes.insert("type".to_string(), json!(col_type));
            if let Some(description) = col_val.get("description").and_then(Value::as_str) {
                attributes.insert("description".to_string(), json!(description));
            }

            nodes.push(GraphNode {
                id: node_id.clone(),
                kind: role,
                attributes,
            });
            node_ids.insert(node_id.clone());
            edges.push(GraphEdge {
                from: node_id,
                to: table.clone(),
                kind: "column_of".to_string(),
                weight: 1.0,
                evidence: "declared".to_string(),
            });
            same_name_groups.entry((col_name.clone(), col_type)).or_default().push((table.clone(), col_name.clone()));
        }
    }

    for (table, model) in entries {
        let Some(foreign_keys) = model.get("foreign_keys").and_then(Value::as_array) else {
            continue;
        };
        for fk in foreign_keys {
            let (Some(column), Some(references_table), Some(references_column)) = (
                fk.get("column").and_then(Value::as_str),
                fk.get("references_table").and_then(Value::as_str),
                fk.get("references_column").and_then(Value::as_str),
            ) else {
                continue;
            };
            let from = format!("{table}.{column}");
            let to = format!("{references_table}.{references_column}");
            // Only when both ends are actual nodes in this graph -- the
            // referenced table might not have a `table_models` row of its
            // own yet (never `describe`d), in which case there is no node
            // to point the edge at.
            if node_ids.contains(&from) && node_ids.contains(&to) {
                edges.push(GraphEdge {
                    from,
                    to,
                    kind: "foreign_key".to_string(),
                    weight: 1.0,
                    evidence: "detected".to_string(),
                });
            }
        }
    }

    for members in same_name_groups.into_values() {
        if members.len() < 2 {
            continue;
        }
        for i in 0..members.len() {
            for j in (i + 1)..members.len() {
                let (table_a, col_a) = &members[i];
                let (table_b, col_b) = &members[j];
                if table_a == table_b {
                    continue;
                }
                // Deterministic ordering (lower table name first) so
                // rebuilding from the same models never reorders this
                // edge's own `from`/`to`.
                let ((from_t, from_c), (to_t, to_c)) =
                    if table_a <= table_b { ((table_a, col_a), (table_b, col_b)) } else { ((table_b, col_b), (table_a, col_a)) };
                edges.push(GraphEdge {
                    from: format!("{from_t}.{from_c}"),
                    to: format!("{to_t}.{to_c}"),
                    kind: "same_name".to_string(),
                    weight: 0.5,
                    evidence: "same_name".to_string(),
                });
            }
        }
    }

    ConceptGraph { nodes, edges }
}

/// Reads every stored `table_models` row for `tenant_id`, merges each
/// one's annotations the same way `describe` does, and builds the graph --
/// [`get_or_build_graph`]'s bootstrap path and [`tick_once`]'s rebuild path
/// both funnel through this.
async fn build_graph_for_tenant(state: &AppState, tenant_id: i64) -> Result<ConceptGraph, AppError> {
    let models = state.db.list_table_models(tenant_id).await?;
    let mut entries = Vec::with_capacity(models.len());
    for row in models {
        let model: Value = serde_json::from_str(&row.model_json)
            .map_err(|e| AppError::Internal(format!("stored table model_json is corrupt: {e}")))?;
        let annotations = state.db.list_table_model_annotations(tenant_id, row.table_name.clone()).await?;
        let merged = crate::tables_model::merge_annotations(model, &annotations);
        entries.push((row.table_name, merged));
    }
    Ok(build_graph_from_models(&entries))
}

/// requirement 3: the latest graph for this tenant, building and storing a
/// fresh one (version 1, not stale) the first time any of the three tools
/// is ever called -- same bootstrap-on-first-call convention
/// `tables_model::table_describe` uses for `table_models`, so a tenant
/// that has never had a tick run yet still gets an answer instead of an
/// empty graph.
async fn get_or_build_graph(state: &AppState, tenant: &Tenant) -> Result<(ConceptGraph, i64, bool, i64), AppError> {
    match state.db.get_table_graph(tenant.id).await? {
        Some(row) => {
            let graph: ConceptGraph = serde_json::from_str(&row.graph_json)
                .map_err(|e| AppError::Internal(format!("stored table graph_json is corrupt: {e}")))?;
            Ok((graph, row.version, row.stale, row.computed_at))
        }
        None => {
            let graph = build_graph_for_tenant(state, tenant.id).await?;
            let graph_json = serde_json::to_string(&graph)
                .map_err(|e| AppError::Internal(format!("table graph serialize: {e}")))?;
            state.db.upsert_table_graph(tenant.id, 1, graph_json).await?;
            Ok((graph, 1, false, crate::state::now_unix()))
        }
    }
}

async fn rebuild_tenant_graph(state: &AppState, tenant_id: i64) -> Result<(), AppError> {
    let graph = build_graph_for_tenant(state, tenant_id).await?;
    let graph_json =
        serde_json::to_string(&graph).map_err(|e| AppError::Internal(format!("table graph serialize: {e}")))?;
    let prev_version = state.db.get_table_graph(tenant_id).await?.map(|r| r.version).unwrap_or(0);
    state.db.upsert_table_graph(tenant_id, prev_version + 1, graph_json).await?;
    Ok(())
}

/// requirement 2: rebuilds every tenant whose graph is marked stale.
/// Called from the tail of [`crate::tables_model::tick_once`] (not its own
/// `spawn_tick`) so a graph rebuild always sees that same cycle's freshest
/// models, within the same 30s window. Exposed directly (not just via the
/// caller's own spawn loop) so a test can drive one deterministic cycle,
/// same convention every other `tick_once` in this crate follows.
pub async fn tick_once(state: &AppState) -> Result<(), AppError> {
    let tenant_ids = state.db.list_tenants_with_stale_table_graph().await?;
    for tenant_id in tenant_ids {
        if let Err(e) = rebuild_tenant_graph(state, tenant_id).await {
            tracing::warn!(error = %e, tenant_id, "table graph rebuild failed");
        }
    }
    Ok(())
}

// ---- table-level traversal (join_paths' and the subgraph's shared base) --

/// One `foreign_key`/`same_name` edge, translated from its column-node
/// endpoints down to the tables and columns it connects -- [`table_edges`].
struct TableEdge {
    table_a: String,
    col_a: String,
    table_b: String,
    col_b: String,
    weight: f64,
    evidence: String,
}

/// Splits a node id (`"table"` for a table node, `"table.column"` for a
/// column node) into its owning table -- for a table node this is just the
/// id itself (no `.` to split on), which is exactly the table it names.
fn owning_table(node_id: &str) -> &str {
    node_id.split_once('.').map(|(table, _)| table).unwrap_or(node_id)
}

/// requirement 4: every `foreign_key`/`same_name` edge, translated to the
/// (table, column) pair on each side -- `column_of` edges are irrelevant
/// here (they connect a column to its own table, not two tables).
fn table_edges(graph: &ConceptGraph) -> Vec<TableEdge> {
    graph
        .edges
        .iter()
        .filter(|e| e.kind == "foreign_key" || e.kind == "same_name")
        .filter_map(|e| {
            let (table_a, col_a) = e.from.split_once('.')?;
            let (table_b, col_b) = e.to.split_once('.')?;
            Some(TableEdge {
                table_a: table_a.to_string(),
                col_a: col_a.to_string(),
                table_b: table_b.to_string(),
                col_b: col_b.to_string(),
                weight: e.weight,
                evidence: e.evidence.clone(),
            })
        })
        .collect()
}

/// requirement 3: `host.table.graph(table, hops)`'s subgraph -- every table
/// reachable from `start_table` within `hops` steps over `foreign_key`/
/// `same_name` edges, plus each reached table's own columns (`column_of`)
/// and the connecting edges between them.
fn k_hop_subgraph(graph: &ConceptGraph, start_table: &str, hops: usize) -> (Vec<GraphNode>, Vec<GraphEdge>) {
    let tedges = table_edges(graph);
    let mut table_neighbors: HashMap<&str, Vec<&str>> = HashMap::new();
    for te in &tedges {
        table_neighbors.entry(te.table_a.as_str()).or_default().push(te.table_b.as_str());
        table_neighbors.entry(te.table_b.as_str()).or_default().push(te.table_a.as_str());
    }

    let mut visited: HashSet<&str> = HashSet::new();
    visited.insert(start_table);
    let mut frontier: Vec<&str> = vec![start_table];
    for _ in 0..hops {
        let mut next_frontier = Vec::new();
        for table in &frontier {
            if let Some(neighbors) = table_neighbors.get(table) {
                for neighbor in neighbors {
                    if visited.insert(neighbor) {
                        next_frontier.push(*neighbor);
                    }
                }
            }
        }
        if next_frontier.is_empty() {
            break;
        }
        frontier = next_frontier;
    }

    let nodes: Vec<GraphNode> = graph
        .nodes
        .iter()
        .filter(|n| visited.contains(owning_table(&n.id)))
        .cloned()
        .collect();
    let edges: Vec<GraphEdge> = graph
        .edges
        .iter()
        .filter(|e| match e.kind.as_str() {
            "column_of" => visited.contains(e.to.as_str()),
            _ => visited.contains(owning_table(&e.from)) && visited.contains(owning_table(&e.to)),
        })
        .cloned()
        .collect();
    (nodes, edges)
}

/// requirement 3: `host.table.graph(table?, hops?)` -- the whole graph when
/// `table` is omitted, or the [`k_hop_subgraph`] around it otherwise.
/// requirement 6: reads only `tenant`'s own graph.
pub async fn table_graph(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let table = tables::arg_str_opt(args, "table");
    let hops = args.get("hops").and_then(Value::as_i64).unwrap_or(1).clamp(1, MAX_HOPS) as usize;

    let (graph, version, stale, computed_at) = get_or_build_graph(state, tenant).await?;

    let (nodes, edges) = match &table {
        Some(t) => {
            if !graph.nodes.iter().any(|n| n.kind == "table" && n.id == *t) {
                return Err(tables::table_not_found(t));
            }
            k_hop_subgraph(&graph, t, hops)
        }
        None => (graph.nodes, graph.edges),
    };

    Ok(json!({
        "schema": GRAPH_SCHEMA,
        "version": version,
        "stale": stale,
        "computed_at": computed_at,
        "nodes": nodes,
        "edges": edges,
    }))
}

// ---- join_paths -----------------------------------------------------------

/// One hop of a join path -- [`table_edges`]'s `TableEdge`, oriented the
/// way this particular path traverses it (its `left_*` is always the table
/// the path is departing, `right_*` the table it arrives at, regardless of
/// which side the underlying edge happened to declare first).
#[derive(Clone)]
struct PathStep {
    left_table: String,
    left_column: String,
    right_table: String,
    right_column: String,
    weight: f64,
    evidence: String,
}

struct Neighbor {
    to_table: String,
    col_from: String,
    col_to: String,
    weight: f64,
    evidence: String,
}

/// Builds a table -> neighbors adjacency list from every `TableEdge`, one
/// [`Neighbor`] entry per direction -- so a path step departing either side
/// of an edge already carries the right `col_from`/`col_to` without the
/// traversal itself needing to know which side the edge was declared from.
fn build_adjacency(tedges: &[TableEdge]) -> HashMap<String, Vec<Neighbor>> {
    let mut adjacency: HashMap<String, Vec<Neighbor>> = HashMap::new();
    for te in tedges {
        adjacency.entry(te.table_a.clone()).or_default().push(Neighbor {
            to_table: te.table_b.clone(),
            col_from: te.col_a.clone(),
            col_to: te.col_b.clone(),
            weight: te.weight,
            evidence: te.evidence.clone(),
        });
        adjacency.entry(te.table_b.clone()).or_default().push(Neighbor {
            to_table: te.table_a.clone(),
            col_from: te.col_b.clone(),
            col_to: te.col_a.clone(),
            weight: te.weight,
            evidence: te.evidence.clone(),
        });
    }
    adjacency
}

/// The fixed inputs [`dfs_paths`]' recursion never mutates -- bundled so
/// each recursive call needs only this one extra argument alongside its
/// own `current`/`remaining_hops` and the mutable [`DfsState`].
struct PathSearch<'a> {
    adjacency: &'a HashMap<String, Vec<Neighbor>>,
    to: &'a str,
}

/// The mutable working set [`dfs_paths`]' recursion threads through --
/// bundled for the same reason as [`PathSearch`], keeping the function
/// under clippy's argument-count lint without losing the "each branch pops
/// what it pushed" backtracking shape.
struct DfsState<'a> {
    visited: &'a mut HashSet<String>,
    steps: &'a mut Vec<PathStep>,
    results: &'a mut Vec<Vec<PathStep>>,
}

fn dfs_paths(search: &PathSearch, current: &str, remaining_hops: usize, state: &mut DfsState) {
    let Some(neighbors) = search.adjacency.get(current) else {
        return;
    };
    for n in neighbors {
        if state.visited.contains(&n.to_table) {
            continue;
        }
        state.steps.push(PathStep {
            left_table: current.to_string(),
            left_column: n.col_from.clone(),
            right_table: n.to_table.clone(),
            right_column: n.col_to.clone(),
            weight: n.weight,
            evidence: n.evidence.clone(),
        });
        if n.to_table == search.to {
            state.results.push(state.steps.clone());
        } else if remaining_hops > 1 {
            state.visited.insert(n.to_table.clone());
            dfs_paths(search, &n.to_table, remaining_hops - 1, state);
            state.visited.remove(&n.to_table);
        }
        state.steps.pop();
    }
}

/// requirement 4: up to [`MAX_JOIN_PATHS`] shortest paths (by step count,
/// ties broken by higher confidence) from `from` to `to` over
/// `foreign_key`/`same_name` edges.
fn find_paths(graph: &ConceptGraph, from: &str, to: &str) -> Vec<Vec<PathStep>> {
    let tedges = table_edges(graph);
    let adjacency = build_adjacency(&tedges);
    let mut results = Vec::new();
    let mut visited = HashSet::new();
    visited.insert(from.to_string());
    let mut steps = Vec::new();
    let search = PathSearch { adjacency: &adjacency, to };
    let mut state = DfsState { visited: &mut visited, steps: &mut steps, results: &mut results };
    dfs_paths(&search, from, MAX_PATH_HOPS, &mut state);
    results.sort_by(|a, b| {
        let confidence_a: f64 = a.iter().map(|s| s.weight).product();
        let confidence_b: f64 = b.iter().map(|s| s.weight).product();
        a.len().cmp(&b.len()).then_with(|| confidence_b.partial_cmp(&confidence_a).unwrap_or(std::cmp::Ordering::Equal))
    });
    results.truncate(MAX_JOIN_PATHS);
    results
}

fn build_sql_join(steps: &[PathStep]) -> String {
    let mut parts = Vec::with_capacity(steps.len());
    for (i, step) in steps.iter().enumerate() {
        if i == 0 {
            parts.push(format!(
                "{} JOIN {} ON {}.{} = {}.{}",
                step.left_table, step.right_table, step.left_table, step.left_column, step.right_table, step.right_column
            ));
        } else {
            parts.push(format!(
                "JOIN {} ON {}.{} = {}.{}",
                step.right_table, step.left_table, step.left_column, step.right_table, step.right_column
            ));
        }
    }
    parts.join(" ")
}

/// requirement 4: each table's own candidate join columns (`key`/`id` role)
/// -- `no_path`'s own payload.
fn key_id_candidates(graph: &ConceptGraph, table: &str) -> Vec<String> {
    graph
        .nodes
        .iter()
        .filter(|n| (n.kind == "key" || n.kind == "id") && node_attr_str(n, "table") == table)
        .map(|n| node_attr_str(n, "column").to_string())
        .collect()
}

/// requirement 4: `host.table.join_paths(from, to)`. requirement 6: reads
/// only `tenant`'s own graph.
pub async fn join_paths(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let from = tables::arg_str(args, "from")?;
    let to = tables::arg_str(args, "to")?;

    let (graph, _version, _stale, _computed_at) = get_or_build_graph(state, tenant).await?;
    let paths = find_paths(&graph, &from, &to);

    if paths.is_empty() {
        let mut candidates = Map::new();
        candidates.insert(from.clone(), json!(key_id_candidates(&graph, &from)));
        candidates.insert(to.clone(), json!(key_id_candidates(&graph, &to)));
        return Ok(json!({
            "from": from,
            "to": to,
            "paths": [],
            "no_path": true,
            "candidates": candidates,
        }));
    }

    let paths_json: Vec<Value> = paths
        .iter()
        .map(|steps| {
            let confidence: f64 = steps.iter().map(|s| s.weight).product();
            json!({
                "steps": steps.iter().map(|s| json!({
                    "left_table": s.left_table,
                    "left_column": s.left_column,
                    "right_table": s.right_table,
                    "right_column": s.right_column,
                    "evidence": s.evidence,
                })).collect::<Vec<_>>(),
                "sql_join": build_sql_join(steps),
                "confidence": confidence,
            })
        })
        .collect();

    Ok(json!({"from": from, "to": to, "paths": paths_json}))
}

// ---- next_questions ---------------------------------------------------

fn first_col_with_role<'a>(graph: &'a ConceptGraph, table: &str, role: &str) -> Option<&'a GraphNode> {
    graph.nodes.iter().find(|n| n.kind == role && node_attr_str(n, "table") == table)
}

/// requirement 8's substitution: a column's annotated `description` when
/// one exists (AC8: `next_questions` must use it in the question text),
/// its bare column name otherwise.
fn label(node: &GraphNode) -> String {
    node.attributes
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_else(|| node_attr_str(node, "column"))
        .to_string()
}

fn col_name(node: &GraphNode) -> String {
    node_attr_str(node, "column").to_string()
}

/// requirement 5's fourth template: a table this `foreign_key`-reachable
/// (one hop) from `table`, that itself has a `category`-role column --
/// `(neighbor_table, this_table's_fk_column, neighbor's_referenced_column,
/// neighbor's_category_node)`.
fn joined_category_neighbor<'a>(graph: &'a ConceptGraph, table: &str) -> Option<(String, String, String, &'a GraphNode)> {
    let prefix = format!("{table}.");
    for e in &graph.edges {
        if e.kind != "foreign_key" {
            continue;
        }
        let (this_col, neighbor_id) = if let Some(c) = e.from.strip_prefix(&prefix) {
            (c, &e.to)
        } else if let Some(c) = e.to.strip_prefix(&prefix) {
            (c, &e.from)
        } else {
            continue;
        };
        let Some((neighbor_table, neighbor_col)) = neighbor_id.split_once('.') else {
            continue;
        };
        if neighbor_table == table {
            continue;
        }
        if let Some(category_node) = first_col_with_role(graph, neighbor_table, "category") {
            return Some((neighbor_table.to_string(), this_col.to_string(), neighbor_col.to_string(), category_node));
        }
    }
    None
}

/// requirement 5: `host.table.next_questions(table, limit?)` -- up to
/// [`MAX_NEXT_QUESTIONS`] `{question, sql, connected_via}` entries, one per
/// template, skipping a template whenever the role (or, for the joined
/// template, the foreign-key neighbor) it needs is missing. Every `sql` is
/// parse-checked via the same `sqlparser` call `host.table.query`'s own
/// gate uses (Technical considerations) before it's returned -- a template
/// whose generated SQL somehow fails that check is silently skipped rather
/// than ever handed back unusable SQL. requirement 6: reads only
/// `tenant`'s own graph.
pub async fn next_questions(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let table = tables::arg_str(args, "table")?;
    let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(MAX_NEXT_QUESTIONS as i64).clamp(1, MAX_NEXT_QUESTIONS as i64) as usize;

    let (graph, _version, _stale, _computed_at) = get_or_build_graph(state, tenant).await?;
    if !graph.nodes.iter().any(|n| n.kind == "table" && n.id == table) {
        return Err(tables::table_not_found(&table));
    }

    let measure = first_col_with_role(&graph, &table, "measure");
    let category = first_col_with_role(&graph, &table, "category");
    let date = first_col_with_role(&graph, &table, "date");
    let id_like = first_col_with_role(&graph, &table, "key").or_else(|| first_col_with_role(&graph, &table, "id"));

    let mut questions: Vec<Value> = Vec::new();

    // 1. total of a measure by a category.
    if let (Some(m), Some(c)) = (measure, category) {
        let sql = format!(
            "SELECT \"{cat}\" AS category, SUM(\"{meas}\") AS total FROM \"{table}\" GROUP BY \"{cat}\"",
            cat = col_name(c),
            meas = col_name(m),
        );
        if tables::validate_query_structure(&sql).is_ok() {
            questions.push(json!({
                "question": format!("Total {} by {}", label(m), label(c)),
                "sql": sql,
                "connected_via": Value::Null,
            }));
        }
    }

    // 2. a measure over a date column.
    if let (Some(m), Some(d)) = (measure, date) {
        let sql = format!(
            "SELECT \"{date}\" AS period, SUM(\"{meas}\") AS total FROM \"{table}\" GROUP BY \"{date}\" ORDER BY \"{date}\"",
            date = col_name(d),
            meas = col_name(m),
        );
        if tables::validate_query_structure(&sql).is_ok() {
            questions.push(json!({
                "question": format!("{} over time by {}", label(m), label(d)),
                "sql": sql,
                "connected_via": Value::Null,
            }));
        }
    }

    // 3. top 10 ids by a measure.
    if let (Some(m), Some(k)) = (measure, id_like) {
        let sql = format!(
            "SELECT \"{key}\", \"{meas}\" FROM \"{table}\" ORDER BY \"{meas}\" DESC LIMIT 10",
            key = col_name(k),
            meas = col_name(m),
        );
        if tables::validate_query_structure(&sql).is_ok() {
            questions.push(json!({
                "question": format!("Top 10 {} by {}", label(k), label(m)),
                "sql": sql,
                "connected_via": Value::Null,
            }));
        }
    }

    // 4. a measure by a category on a joined table (via a foreign-key path).
    if let Some(m) = measure
        && let Some((neighbor, fk_col, fk_ref_col, category_node)) = joined_category_neighbor(&graph, &table)
    {
        let sql = format!(
            "SELECT t2.\"{cat}\" AS category, SUM(t1.\"{meas}\") AS total FROM \"{table}\" t1 \
             JOIN \"{neighbor}\" t2 ON t1.\"{fk}\" = t2.\"{fk_ref}\" GROUP BY t2.\"{cat}\"",
            cat = col_name(category_node),
            meas = col_name(m),
            fk = fk_col,
            fk_ref = fk_ref_col,
        );
        if tables::validate_query_structure(&sql).is_ok() {
            questions.push(json!({
                "question": format!("Total {} by {}.{}", label(m), neighbor, label(category_node)),
                "sql": sql,
                "connected_via": {"table": neighbor, "column": fk_col, "evidence": "detected"},
            }));
        }
    }

    // 5. count of rows by category.
    if let Some(c) = category {
        let sql = format!(
            "SELECT \"{cat}\" AS category, COUNT(*) AS count FROM \"{table}\" GROUP BY \"{cat}\"",
            cat = col_name(c),
        );
        if tables::validate_query_structure(&sql).is_ok() {
            questions.push(json!({
                "question": format!("Count of rows by {}", label(c)),
                "sql": sql,
                "connected_via": Value::Null,
            }));
        }
    }

    questions.truncate(limit);
    Ok(json!({"table": table, "questions": questions}))
}

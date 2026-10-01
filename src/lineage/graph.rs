//! PRD-mcphost-lineage-blast-radius requirement 1: the dependency-graph
//! model, ported from `~/projects/ai-stack @ 4ef6c22` (`lib.rs`'s
//! `LineageGraph`/`ChangeKind`/node kinds) with node kinds rewritten for
//! mcphost's own entities (`{table, column, tool, chain, document, chart,
//! handle, run}` in place of ai-stack's metrics/dashboard kinds) and no
//! federation/domain fields (non-goal: cross-tenant edges are out of scope).

use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// requirement 1: every node id is `<kind>:<name>` (e.g. `table:orders`).
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Table,
    Column,
    Tool,
    Chain,
    Document,
    Chart,
    Handle,
    Run,
}

impl NodeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            NodeKind::Table => "table",
            NodeKind::Column => "column",
            NodeKind::Tool => "tool",
            NodeKind::Chain => "chain",
            NodeKind::Document => "document",
            NodeKind::Chart => "chart",
            NodeKind::Handle => "handle",
            NodeKind::Run => "run",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "table" => NodeKind::Table,
            "column" => NodeKind::Column,
            "tool" => NodeKind::Tool,
            "chain" => NodeKind::Chain,
            "document" => NodeKind::Document,
            "chart" => NodeKind::Chart,
            "handle" => NodeKind::Handle,
            "run" => NodeKind::Run,
            _ => return None,
        })
    }

    /// AC5: every node kind, for an exhaustive effect-table test.
    pub fn all() -> [NodeKind; 8] {
        [
            NodeKind::Table,
            NodeKind::Column,
            NodeKind::Tool,
            NodeKind::Chain,
            NodeKind::Document,
            NodeKind::Chart,
            NodeKind::Handle,
            NodeKind::Run,
        ]
    }
}

/// requirement 1: `ChangeKind {Drop, Rename, TypeChange, Remove, Add}`,
/// ported unchanged from ai-stack.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Drop,
    Rename,
    TypeChange,
    Remove,
    Add,
}

impl ChangeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ChangeKind::Drop => "drop",
            ChangeKind::Rename => "rename",
            ChangeKind::TypeChange => "type_change",
            ChangeKind::Remove => "remove",
            ChangeKind::Add => "add",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "drop" => ChangeKind::Drop,
            "rename" => ChangeKind::Rename,
            "type_change" | "typechange" | "type-change" => ChangeKind::TypeChange,
            "remove" => ChangeKind::Remove,
            "add" => ChangeKind::Add,
            _ => return None,
        })
    }

    /// AC5: every change kind, for an exhaustive effect-table test.
    pub fn all() -> [ChangeKind; 5] {
        [
            ChangeKind::Drop,
            ChangeKind::Rename,
            ChangeKind::TypeChange,
            ChangeKind::Remove,
            ChangeKind::Add,
        ]
    }
}

/// requirement 2: the effect table's per-consumer ranking bucket.
/// Declaration order matters -- `derive(Ord)` ranks later variants higher,
/// and [`LineageGraph::blast_radius`] sorts by severity descending
/// (`Breaking` first).
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    None,
    Degrading,
    Breaking,
}

/// requirement 1: each node carries `label`, `uses`, `created_unix`,
/// `last_used_unix`. `orphaned` (requirement 7) is `false` until a
/// confirmed `host.table.drop` cascade flags a dependent chart/handle node.
#[derive(Debug, Clone, Serialize)]
pub struct LineageNode {
    pub id: String,
    pub kind: NodeKind,
    pub label: String,
    pub uses: u64,
    pub created_unix: i64,
    pub last_used_unix: Option<i64>,
    pub orphaned: bool,
}

/// requirement 6: one entry of `ImpactReport::impacted`, ported from
/// ai-stack's `ImpactedNode`. `cross_domain` is always `false` -- the
/// non-goal drops federation/domain edges, but the field stays so the
/// response shape matches the ported type.
#[derive(Debug, Clone, Serialize)]
pub struct ImpactedNode {
    pub id: String,
    pub kind: NodeKind,
    pub label: String,
    pub depth: usize,
    pub effect: &'static str,
    pub severity: Severity,
    pub action: &'static str,
    pub uses: u64,
    pub cross_domain: bool,
}

/// requirement 6: `blast_radius`'s return shape, ported from ai-stack's
/// `ImpactReport`.
#[derive(Debug, Clone, Serialize)]
pub struct ImpactReport {
    pub impacted: Vec<ImpactedNode>,
    pub truncated: bool,
}

/// requirement 8: one entry of a `trace` result (either direction).
/// `evidence` (requirement 4/AC1: "an edge ... exists with evidence
/// source_scan") is only ever `Some` for a direct (`depth == 1`) entry --
/// a multi-hop entry was reached across more than one edge, each of which
/// may carry different evidence, so there's no single value to report.
#[derive(Debug, Clone, Serialize)]
pub struct TraceEntry {
    pub id: String,
    pub kind: NodeKind,
    pub label: String,
    pub depth: usize,
    pub evidence: Option<String>,
}

/// requirement 1: the dependency graph itself -- nodes plus upstream/
/// downstream adjacency, ported from ai-stack's `LineageGraph` (a
/// `BTreeMap` of nodes plus upstream/downstream adjacency).
#[derive(Debug, Clone, Default)]
pub struct LineageGraph {
    pub nodes: BTreeMap<String, LineageNode>,
    /// `upstream_id -> {downstream_id, ...}` -- "these nodes depend on me".
    pub downstream: BTreeMap<String, BTreeSet<String>>,
    /// `downstream_id -> {upstream_id, ...}` -- "I depend on these nodes".
    pub upstream: BTreeMap<String, BTreeSet<String>>,
    /// `(upstream_id, downstream_id) -> evidence` for every direct edge.
    pub edge_evidence: BTreeMap<(String, String), String>,
}

impl LineageGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a node, or (if one with this id already exists) refreshes
    /// only `kind`/`label` -- `uses`/`created_unix`/`last_used_unix`/
    /// `orphaned` come from the persisted row and must never be reset by a
    /// re-registration.
    pub fn upsert_node(&mut self, node: LineageNode) {
        match self.nodes.get_mut(&node.id) {
            Some(existing) => {
                existing.kind = node.kind;
                existing.label = node.label;
            }
            None => {
                self.nodes.insert(node.id.clone(), node);
            }
        }
    }

    pub fn add_edge(&mut self, upstream_id: &str, downstream_id: &str, evidence: &str) {
        self.downstream
            .entry(upstream_id.to_string())
            .or_default()
            .insert(downstream_id.to_string());
        self.upstream
            .entry(downstream_id.to_string())
            .or_default()
            .insert(upstream_id.to_string());
        self.edge_evidence
            .insert((upstream_id.to_string(), downstream_id.to_string()), evidence.to_string());
    }

    fn bfs(&self, start: &str, adjacency: &BTreeMap<String, BTreeSet<String>>, max_depth: usize) -> Vec<(String, usize)> {
        let mut visited: BTreeMap<String, usize> = BTreeMap::new();
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();
        if let Some(children) = adjacency.get(start) {
            for c in children {
                queue.push_back((c.clone(), 1));
            }
        }
        while let Some((cur, depth)) = queue.pop_front() {
            if visited.contains_key(&cur) {
                continue;
            }
            visited.insert(cur.clone(), depth);
            if depth >= max_depth {
                continue;
            }
            if let Some(children) = adjacency.get(&cur) {
                for c in children {
                    if !visited.contains_key(c) {
                        queue.push_back((c.clone(), depth + 1));
                    }
                }
            }
        }
        visited.into_iter().collect()
    }

    /// requirement 6: ported `blast_radius(name, ChangeKind, max_depth,
    /// top_n) -> ImpactReport`. Ranked by severity (descending), then
    /// `uses` (descending), then `depth` (ascending), then `id` (for a
    /// deterministic tie-break) -- AC3's own ordering.
    pub fn blast_radius(&self, id: &str, change_kind: ChangeKind, max_depth: usize, top_n: usize) -> ImpactReport {
        let reached = self.bfs(id, &self.downstream, max_depth);
        let mut impacted: Vec<ImpactedNode> = reached
            .into_iter()
            .filter_map(|(nid, depth)| {
                let node = self.nodes.get(&nid)?;
                let (effect, severity, action) = super::effects::effect_for(change_kind, node.kind);
                Some(ImpactedNode {
                    id: nid,
                    kind: node.kind,
                    label: node.label.clone(),
                    depth,
                    effect,
                    severity,
                    action,
                    uses: node.uses,
                    cross_domain: false,
                })
            })
            .collect();
        impacted.sort_by(|a, b| {
            b.severity
                .cmp(&a.severity)
                .then(b.uses.cmp(&a.uses))
                .then(a.depth.cmp(&b.depth))
                .then(a.id.cmp(&b.id))
        });
        let truncated = impacted.len() > top_n;
        impacted.truncate(top_n);
        ImpactReport { impacted, truncated }
    }

    /// requirement 8: full transitive upstream and downstream lists for
    /// `id`, sorted by depth then id. No depth cap -- a trace answers
    /// "everything", unlike `blast_radius`'s bounded, ranked report. The
    /// over-100-downstream handle (AC7) is the caller's own pagination,
    /// not a truncation here.
    pub fn trace(&self, id: &str) -> (Vec<TraceEntry>, Vec<TraceEntry>) {
        let to_entries = |reached: Vec<(String, usize)>, edge_key: &dyn Fn(&str) -> (String, String)| -> Vec<TraceEntry> {
            let mut entries: Vec<TraceEntry> = reached
                .into_iter()
                .filter_map(|(nid, depth)| {
                    let node = self.nodes.get(&nid)?;
                    let evidence = (depth == 1).then(|| edge_key(&nid)).and_then(|k| self.edge_evidence.get(&k).cloned());
                    Some(TraceEntry {
                        id: nid,
                        kind: node.kind,
                        label: node.label.clone(),
                        depth,
                        evidence,
                    })
                })
                .collect();
            entries.sort_by(|a, b| a.depth.cmp(&b.depth).then(a.id.cmp(&b.id)));
            entries
        };
        let upstream = to_entries(self.bfs(id, &self.upstream, usize::MAX), &|nid: &str| (nid.to_string(), id.to_string()));
        let downstream = to_entries(self.bfs(id, &self.downstream, usize::MAX), &|nid: &str| (id.to_string(), nid.to_string()));
        (upstream, downstream)
    }
}

pub fn node_id(kind: NodeKind, name: &str) -> String {
    format!("{}:{}", kind.as_str(), name)
}

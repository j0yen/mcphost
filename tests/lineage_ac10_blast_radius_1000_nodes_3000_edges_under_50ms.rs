//! PRD-mcphost-lineage-blast-radius AC10 (P0) -- Given 1,000 nodes and 3,000 edges, When blast radius runs,
//! Then it completes under 50 ms on the builder.

use mcphost::lineage::graph::{ChangeKind, LineageGraph, LineageNode, NodeKind, node_id};
use std::collections::HashSet;
use std::time::Instant;

fn tool_id(i: usize) -> String {
    node_id(NodeKind::Tool, &format!("n{i}"))
}

#[test]
fn blast_radius_over_1000_nodes_3000_edges_completes_under_50ms() {
    let mut graph = LineageGraph::new();
    let root_id = node_id(NodeKind::Table, "root");
    graph.upsert_node(LineageNode {
        id: root_id.clone(),
        kind: NodeKind::Table,
        label: "root".to_string(),
        uses: 0,
        created_unix: 0,
        last_used_unix: None,
        orphaned: false,
    });
    for i in 1..1000 {
        graph.upsert_node(LineageNode {
            id: tool_id(i),
            kind: NodeKind::Tool,
            label: format!("n{i}"),
            uses: (i % 50) as u64,
            created_unix: 0,
            last_used_unix: None,
            orphaned: false,
        });
    }
    assert_eq!(graph.nodes.len(), 1000);

    let mut edges: HashSet<(String, String)> = HashSet::new();
    let add = |up: String, down: String, edges: &mut HashSet<(String, String)>, graph: &mut LineageGraph| {
        if up != down && edges.insert((up.clone(), down.clone())) {
            graph.add_edge(&up, &down, "source_scan");
        }
    };

    // A ternary spanning tree out of `root` so every one of the 999 tool
    // nodes is reachable (999 edges).
    for i in 1..1000 {
        let parent = if i <= 3 { root_id.clone() } else { tool_id((i - 1) / 3) };
        add(parent, tool_id(i), &mut edges, &mut graph);
    }

    // Pad out to 3,000 edges with extra (acyclic-by-construction: always
    // lower index -> higher index) cross edges.
    let mut i = 1usize;
    let mut step = 37usize;
    while edges.len() < 3000 {
        let a = i % 999 + 1;
        let b = (i * step + 17) % 999 + 1;
        let (lo, hi) = if a < b { (a, b) } else { (b, a) };
        if lo != hi {
            add(tool_id(lo), tool_id(hi), &mut edges, &mut graph);
        }
        i += 1;
        if i.is_multiple_of(999) {
            step += 1;
        }
    }
    assert_eq!(edges.len(), 3000);

    let start = Instant::now();
    let report = graph.blast_radius(&root_id, ChangeKind::Drop, usize::MAX, usize::MAX);
    let elapsed = start.elapsed();

    assert_eq!(report.impacted.len(), 999, "every tool node must be reachable from root");
    assert!(
        elapsed.as_millis() < 50,
        "blast_radius over 1,000 nodes / 3,000 edges took {elapsed:?}, must be under 50ms"
    );
}

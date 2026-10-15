//! PRD-mcphost-emitted-doc-pointers-resolve
//! AC1 (P0) — Given the 12 `.md#` literals, the 16 `doc_link` values and
//! `control.rs:1107`, When each is resolved against the headings `build.rs`
//! found, Then >= 10 resolve today and the test prints every non-resolving
//! pointer with its file and line.

use mcphost::doc_anchors::{DocAnchor, slugify};

/// Every pointer the host printed before this PRD, as `(file, line, pointer)`
/// at the commit the PRD was drafted against: the `doc_link` rows of
/// `src/help.rs` (the 12 `.md#` literals among them) and the recipe `docs`
/// field in `src/control.rs` (`host.quickstart`, line 1107 in the PRD).
const ORIGINAL_POINTERS: &[(&str, u32, &str)] = &[
    ("src/help.rs", 68, "docs/agent-quickstart.md#quickstart-for-agents"),
    ("src/help.rs", 75, "docs/agent-quickstart.md#quickstart-for-agents"),
    ("src/help.rs", 82, "docs/agent-quickstart.md#quickstart-for-agents"),
    ("src/help.rs", 89, "www/support.html"),
    ("src/help.rs", 96, "www/status.html"),
    ("src/help.rs", 103, "docs/agent-quickstart.md#quickstart-for-agents"),
    ("src/help.rs", 110, "README.md#kinds"),
    ("src/help.rs", 117, "README.md#call-limits"),
    ("src/help.rs", 124, "docs/agent-quickstart.md#quickstart-for-agents"),
    ("src/help.rs", 131, "docs/agent-quickstart.md#quickstart-for-agents"),
    ("src/help.rs", 138, "README.md#egress-proxy-network-public--egress"),
    ("src/help.rs", 145, "docs/agent-quickstart.md#quickstart-for-agents"),
    ("src/help.rs", 152, "docs/agent-quickstart.md#quickstart-for-agents"),
    ("src/help.rs", 159, "docs/agent-quickstart.md#quickstart-for-agents"),
    ("src/help.rs", 166, "www/status.html"),
    ("src/help.rs", 173, "www/status.html"),
    ("src/help.rs", 180, "docs/tools.md"),
    ("src/help.rs", 187, "docs/tools.md"),
    ("src/help.rs", 194, "docs/tools.md"),
    ("src/help.rs", 201, "docs/plans.md"),
    ("src/help.rs", 208, "docs/protocol/capabilities.md"),
    ("src/control.rs", 1107, "host.quickstart"),
];

/// A pointer resolves when it names a heading `build.rs` found; a bare tool
/// name resolves through its `### \`<tool>\`` heading in `docs/tools.md`.
fn resolves(pointer: &str) -> bool {
    if pointer.contains(".md#") {
        DocAnchor::parse(pointer).is_some()
    } else if pointer.starts_with("host.") || pointer.starts_with("billing.") {
        DocAnchor::parse(&format!("docs/tools.md#{}", slugify(pointer))).is_some()
    } else {
        false
    }
}

#[test]
fn edp_ac01_at_least_ten_original_pointers_resolve_and_the_rest_are_printed() {
    let (ok, bad): (Vec<_>, Vec<_>) = ORIGINAL_POINTERS.iter().partition(|(_, _, p)| resolves(p));
    for (file, line, pointer) in &bad {
        println!("unresolved pointer: {file}:{line} {pointer}");
    }
    println!("resolved {} of {}", ok.len(), ORIGINAL_POINTERS.len());
    assert!(ok.len() >= 10, "fewer than 10 emitted pointers resolve: {} of {}", ok.len(), ORIGINAL_POINTERS.len());
    // The falsifier counts only `.md#` literals: all 12 are listed above.
    let literals = ORIGINAL_POINTERS.iter().filter(|(_, _, p)| p.contains(".md#")).count();
    assert_eq!(literals, 12, "the PRD names 12 `.md#` literals");
}

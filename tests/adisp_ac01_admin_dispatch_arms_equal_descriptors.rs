//! Hand fix (rb seat 10-19): `admin.funnel` was dispatched but never listed.
//! Drift contract: every `"admin.…" =>` arm of `dispatch_admin_tool` in
//! `src/handler.rs` is an `admin_tool_descriptors()` name and every
//! descriptor name is an arm. The dispatch `match` stays a literal `match`
//! (a table or boxed arms would regrow the fwdtab dispatch stack frames), so
//! the arm names are read from its source text, the same way fwdtab_ac03 and
//! vbd_ac06 read `handler.rs`.
//!
//! The reverse direction in the runtime sense (a descriptor whose schema-valid
//! call reaches no handler) is proven by vbd_ac07; this file adds the
//! name-set equality that catches the arm with no descriptor, which tools/list
//! cannot show and no schema-driven test can reach.

use mcphost::handler::admin_tool_descriptors;
use std::collections::BTreeSet;

/// The text of `async fn dispatch_admin_tool`, up to its `other =>` fallthrough.
fn dispatcher_source(src: &str) -> &str {
    let start = src.find("async fn dispatch_admin_tool(").expect("dispatch_admin_tool in handler.rs");
    let rest = &src[start..];
    let end = rest.find("other => Err(AppError::ToolNotFound").expect("the ToolNotFound fallthrough arm");
    &rest[..end]
}

/// Every `"admin.…" =>` arm name in the dispatcher text (one arm per line, the
/// shape the file uses; a comment line never matches because it starts `//`).
fn arm_names(dispatcher: &str) -> BTreeSet<String> {
    dispatcher
        .lines()
        .map(str::trim_start)
        .filter_map(|l| l.strip_prefix("\"admin."))
        .filter_map(|rest| {
            let (name, tail) = rest.split_once('"')?;
            tail.trim_start().starts_with("=>").then(|| format!("admin.{name}"))
        })
        .collect()
}

/// Readable diff of the two name sets, empty when they are equal.
fn drift(arms: &BTreeSet<String>, descriptors: &BTreeSet<String>) -> Vec<String> {
    let mut out = Vec::new();
    for n in arms.difference(descriptors) {
        out.push(format!("dispatch arm with no descriptor (tools/list never shows it): {n}"));
    }
    for n in descriptors.difference(arms) {
        out.push(format!("descriptor with no dispatch arm (a call reaches ToolNotFound): {n}"));
    }
    out
}

#[test]
fn adisp_ac01_admin_dispatch_arms_equal_admin_tool_descriptors() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/handler.rs")).expect("read handler.rs");
    let arms = arm_names(dispatcher_source(&src));
    let descriptors: BTreeSet<String> = admin_tool_descriptors().into_iter().map(|t| t.name.to_string()).collect();
    assert!(arms.len() > 50, "the arm scan found only {} arms; the parser has drifted from the file: {arms:?}", arms.len());
    let diff = drift(&arms, &descriptors);
    assert!(diff.is_empty(), "admin dispatch arms and admin descriptors disagree:\n{}", diff.join("\n"));
}

#[test]
fn adisp_ac01_a_planted_arm_without_a_descriptor_is_named() {
    let fixture = "async fn dispatch_admin_tool(&self) {\n    match name {\n        // a comment\n        \"admin.a\" => x().await,\n        \"admin.b.c\" => {\n            y()\n        }\n        \"admin.ghost\" => z().await,\n        other => Err(AppError::ToolNotFound(other.to_string())),\n    }\n}\n";
    let arms = arm_names(dispatcher_source(fixture));
    assert_eq!(arms.iter().map(String::as_str).collect::<Vec<_>>(), ["admin.a", "admin.b.c", "admin.ghost"]);
    let descriptors: BTreeSet<String> = ["admin.a", "admin.b.c", "admin.orphan"].into_iter().map(String::from).collect();
    let diff = drift(&arms, &descriptors);
    assert_eq!(diff.len(), 2, "{diff:?}");
    assert!(diff[0].contains("no descriptor") && diff[0].ends_with("admin.ghost"), "{diff:?}");
    assert!(diff[1].contains("no dispatch arm") && diff[1].ends_with("admin.orphan"), "{diff:?}");
}

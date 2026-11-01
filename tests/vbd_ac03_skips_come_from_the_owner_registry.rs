//! contract: every skip validate_host_args makes is a registration the owning module exported, and no hand skip remains in src/
//! PRD-mcphost-host-args-validated-by-default
//! AC3 (P0) -- Given `host_args.rs`, When its skip logic is tested, Then every
//! skip `validate_host_args` makes comes from `arg_owners()` (registrations
//! exported by `paged` and `errors`), a violation at an owned path is skipped
//! only on a tool the owner applies to and refused on any other, and a source
//! scan finds no `left_to_projection`, no `remove("maximum")` and no literal
//! `"additionalProperties"` compare outside the registry reader.

use mcphost::handler::host_tool_descriptors;
use mcphost::host_args::{ArgOwner, arg_owners, owned_by};
use mcphost::precheck::precheck_args as validate_host_args;
use mcphost::kinds::KindRegistry;
use mcphost::paged::{fields_for, paged_spec};
use serde_json::{Value, json};

#[test]
fn vbd_ac03_the_registry_holds_what_paged_and_errors_export() {
    let owners = arg_owners();
    let triples: Vec<(&str, &[&str], &[&str])> = owners.iter().map(|o| (o.path, o.keywords, o.codes)).collect();
    // Every owner a module exports is in the registry (paged and errors, then the
    // plan-ceiling and spec-test owners), in that order and nothing else.
    let exported = mcphost::paged::ARG_OWNERS.len()
        + mcphost::errors::ARG_OWNERS.len()
        + mcphost::plans::ARG_OWNERS.len()
        + mcphost::handler::ARG_OWNERS.len();
    assert_eq!(owners.len(), exported, "{triples:?}");
    for (path, keywords, codes) in [
        ("/fields", &["*"][..], &["field_unknown", "args_invalid"][..]),
        ("/limit", &["maximum"][..], &[][..]),
        ("", &["additionalProperties"][..], &["unknown_argument"][..]),
    ] {
        assert!(triples.contains(&(path, keywords, codes)), "missing owner {path:?}: {triples:?}");
    }
}

#[test]
fn vbd_ac03_an_owned_violation_is_skipped_only_where_the_owner_applies() {
    let fixture = ArgOwner {
        applies: |tool| tool == "fixture.owned",
        path: "/fields",
        keywords: &["uniqueItems", "enum"],
        codes: &["field_unknown"],
    };
    let owners = [fixture];
    // Owned path and keyword on the owner's tool, also below the path.
    assert!(owned_by(&owners, "fixture.owned", "/fields", "uniqueItems"));
    assert!(owned_by(&owners, "fixture.owned", "/fields/1", "enum"));
    // Any other tool, path, sibling-prefixed path or keyword is refused.
    assert!(!owned_by(&owners, "fixture.other", "/fields", "uniqueItems"));
    assert!(!owned_by(&owners, "fixture.owned", "/other", "uniqueItems"));
    assert!(!owned_by(&owners, "fixture.owned", "/fields_x", "uniqueItems"));
    assert!(!owned_by(&owners, "fixture.owned", "/fields", "minItems"));
    // The real registry: `maximum` on `limit` is owned for a paged verb only.
    let real = arg_owners();
    assert!(owned_by(real, "host.runs.list", "/limit", "maximum"));
    assert!(!owned_by(real, "host.whoami", "/limit", "maximum"));
    assert!(!owned_by(real, "host.runs.list", "/limit", "minimum"));
    assert!(owned_by(real, "host.whoami", "", "additionalProperties"));
}

#[test]
fn vbd_ac03_the_validator_refuses_an_owned_keyword_on_a_tool_without_that_owner() {
    let kinds = KindRegistry::with_builtin();
    // A paged verb's `limit` above its max is the owner's to clamp.
    let paged = "host.runs.list";
    assert!(paged_spec(paged).is_some());
    validate_host_args(&kinds, paged, &json!({"limit": 5_000_000})).expect("limit above max is clamped, not refused");
    // The same keyword on a property of a verb that does not page is refused.
    let (tool, prop, max) = host_tool_descriptors(&kinds)
        .into_iter()
        .filter(|t| paged_spec(&t.name).is_none() && fields_for(&t.name).is_none() && t.input_schema.get("required").and_then(Value::as_array).is_none_or(|r| r.is_empty()))
        .find_map(|t| {
            let props = t.input_schema.get("properties")?.as_object()?;
            props.iter().find_map(|(k, v)| {
                let max = v.get("maximum")?.as_i64()?;
                (v.get("type")? == "integer").then(|| (t.name.to_string(), k.clone(), max))
            })
        })
        .expect("some non-paged, no-required verb has an integer property with a maximum");
    let err = validate_host_args(&kinds, &tool, &json!({ prop.clone(): max + 1 })).expect_err("must be refused");
    let text = format!("{err:?}");
    assert!(text.contains("maximum"), "{tool}.{prop}: {text}");
}

#[test]
fn vbd_ac03_host_args_has_no_hand_skip() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/host_args.rs")).expect("read host_args.rs");
    let code: Vec<(usize, &str)> =
        src.lines().enumerate().filter(|(_, l)| !l.trim_start().starts_with("//")).map(|(i, l)| (i + 1, l)).collect();
    for needle in ["left_to_projection", "remove(\"maximum\")", "\"additionalProperties\"", "\"/fields\"", "\"maximum\""] {
        let hits: Vec<_> = code.iter().filter(|(_, l)| l.contains(needle)).map(|(n, l)| format!("host_args.rs:{n}: {l}")).collect();
        assert!(hits.is_empty(), "hand skip `{needle}` in host_args.rs:\n{}", hits.join("\n"));
    }
    // Nowhere else in src/ either.
    let mut stack = vec![std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/src"))];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(dir).expect("read src dir").flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                for needle in ["left_to_projection", "remove(\"maximum\")"] {
                    assert!(!text.contains(needle), "`{needle}` in {}", path.display());
                }
            }
        }
    }
}

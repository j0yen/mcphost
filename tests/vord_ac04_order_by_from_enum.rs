//! PRD-mcphost-declared-ordering-per-verb
//! AC4 (P0) -- Given `src/db.rs`, When `vord_ac04_order_by_from_enum` scans
//! the list functions behind `PAGED_VERBS`, Then every `ORDER BY` is produced
//! by `sql_order` and a hand-typed clause fails the test naming the function.

use mcphost::paged::PAGED_VERBS;

const DB_RS: &str = include_str!("../src/db.rs");

/// The `db.rs` functions whose SQL serves each `Paged` verb. A verb added to
/// `PAGED_VERBS` without a row here fails
/// `every_paged_verb_has_its_db_functions_listed`.
const VERB_DB_FNS: &[(&str, &[&str])] = &[
    ("host.channel.read", &["channel_posts_after"]),
    ("host.msg.inbox", &["msg_inbox"]),
    ("host.msg.thread", &["msg_thread"]),
    ("host.docs.list", &["documents_list_live", "documents_list_since"]),
];

/// The text of `fn <name>` in `source`, from its signature to the matching
/// closing brace, with `//` comment lines dropped (prose may say "ORDER BY").
fn fn_body(source: &str, name: &str) -> Option<String> {
    let start = source.find(&format!("fn {name}("))?;
    let rest = &source[start..];
    let open = rest.find('{')?;
    let mut depth = 0usize;
    let mut end = None;
    for (i, ch) in rest[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(open + i + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    let body = &rest[..end?];
    Some(body.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n"))
}

/// One message per function that types an `ORDER BY` by hand or never calls
/// `sql_order`.
fn violations(source: &str, fns: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for name in fns {
        let Some(body) = fn_body(source, name) else {
            out.push(format!("{name}: not found in src/db.rs"));
            continue;
        };
        if body.to_ascii_uppercase().contains("ORDER BY") {
            out.push(format!("{name}: hand-typed ORDER BY; use Paged::sql_order"));
        }
        if !body.contains("sql_order(") {
            out.push(format!("{name}: does not build its ordering with sql_order"));
        }
    }
    out
}

#[test]
fn every_paged_verb_has_its_db_functions_listed() {
    for verb in PAGED_VERBS {
        assert!(
            VERB_DB_FNS.iter().any(|(v, _)| v == verb),
            "{verb} derives Paged but VERB_DB_FNS names no db.rs function for it"
        );
    }
    for (verb, _) in VERB_DB_FNS {
        assert!(PAGED_VERBS.contains(verb), "{verb} is listed here but is not in PAGED_VERBS");
    }
}

#[test]
fn every_order_by_behind_a_paged_verb_is_produced_by_sql_order() {
    let all: Vec<&str> = VERB_DB_FNS.iter().flat_map(|(_, fns)| fns.iter().copied()).collect();
    let found = violations(DB_RS, &all);
    assert!(found.is_empty(), "list queries must take their ORDER BY from sql_order: {found:#?}");
}

#[test]
fn a_hand_typed_order_by_fails_naming_the_function() {
    let doctored = DB_RS.replacen(
        "crate::paged::DocsList::sql_order(crate::paged::DocsOrder::CreatedAsc)",
        "\"ORDER BY seq\"",
        1,
    );
    assert_ne!(doctored, DB_RS, "the doctored replacement must hit documents_list_since");
    let found = violations(&doctored, &["documents_list_since", "msg_inbox"]);
    assert!(
        found.iter().any(|m| m.starts_with("documents_list_since:") && m.contains("hand-typed ORDER BY")),
        "the hand-typed clause must be reported by function name: {found:?}"
    );
    assert!(found.iter().all(|m| !m.starts_with("msg_inbox:")), "an untouched function stays clean: {found:?}");
}

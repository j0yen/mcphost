//! PRD-mcphost-kind-docs-from-registry-b-v2 AC6: every `code: "…"` row a
//! kind's source defines is in that kind's `KindDoc.error_codes` (so on its
//! page) or in `KIND_PAGE_EXEMPT` with a reason; a row planted in a scratch
//! `http.rs` fails naming the code and the kind.

use std::path::{Path, PathBuf};

use mcphost::gendocs::check_kind_error_codes;
use mcphost::kinds::contract::{KIND_PAGE_EXEMPT, UPSTREAM_FAMILY};
use mcphost::kinds::kinddoc::KindDoc;
use mcphost::plans::PlanCatalog;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn docs() -> Vec<KindDoc> {
    let registry = mcphost::kinds::production_registry();
    let plans = PlanCatalog::default_catalog();
    registry.names().into_iter().filter_map(|n| registry.get(n)).map(|k| k.doc(&plans)).collect()
}

fn scratch_with_planted_row(label: &str, row: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kdrb_ac06_{label}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src/kinds")).unwrap();
    for entry in std::fs::read_dir(root().join("src/kinds")).unwrap() {
        let entry = entry.unwrap();
        if entry.path().extension().is_some_and(|e| e == "rs") {
            std::fs::copy(entry.path(), dir.join("src/kinds").join(entry.file_name())).unwrap();
        }
    }
    let http = dir.join("src/kinds/http.rs");
    let mut text = std::fs::read_to_string(&http).unwrap();
    text.push_str(row);
    std::fs::write(&http, text).unwrap();
    dir
}

#[test]
fn kdrb_ac06() {
    // The real tree: every code is on its page or exempt with a reason.
    let docs = docs();
    assert_eq!(check_kind_error_codes(root(), &docs), Ok(()));
    for (kind, code, reason) in KIND_PAGE_EXEMPT {
        assert!(!reason.is_empty(), "exempt {kind}/{code} carries no reason");
        assert!(docs.iter().any(|d| d.name == *kind), "exempt row names an unregistered kind {kind}");
        assert!(
            docs.iter().find(|d| d.name == *kind).unwrap().error_codes.iter().all(|e| e.code != *code),
            "{kind}/{code} is exempt and also listed on its page"
        );
    }

    // Before this PRD the http page listed only the upstream family: the
    // scan names the two codes http defines beyond it.
    let mut before = docs.clone();
    let http = before.iter_mut().find(|d| d.name == "http").unwrap();
    http.error_codes.retain(|e| UPSTREAM_FAMILY.codes.contains(&e.code.as_str()));
    let problems = check_kind_error_codes(root(), &before).unwrap_err();
    for code in ["response_too_large", "template_error"] {
        assert!(problems.iter().any(|p| p.contains(code) && p.contains("kind http")), "{code} not named: {problems:?}");
    }

    // A row planted in a scratch http.rs fails naming the code and the kind.
    let planted = "\ncrate::err_code! {\n    pub UPSTREAM_SLOW,\n    code: \"upstream_slow\", family: Upstream, class: UpstreamFault, retryable: true,\n    remedy: \"Retry later.\",\n}\n";
    let scratch = scratch_with_planted_row("planted", planted);
    let problems = check_kind_error_codes(&scratch, &docs).unwrap_err();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("upstream_slow") && problems[0].contains("kind http"), "{problems:?}");
    let _ = std::fs::remove_dir_all(&scratch);

    // The same row is fine once it is exempt or listed: the check follows the docs it is given.
    let mut listed = docs;
    listed.iter_mut().find(|d| d.name == "http").unwrap().error_codes.push(mcphost::kinds::kinddoc::ErrorDoc {
        code: "upstream_slow".to_string(),
        when: "planted".to_string(),
    });
    let scratch = scratch_with_planted_row("listed", planted);
    assert_eq!(check_kind_error_codes(&scratch, &listed), Ok(()));
    let _ = std::fs::remove_dir_all(&scratch);
}

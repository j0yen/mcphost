//! PRD-mcphost-kind-docs-from-registry AC2: `mcphost gen-docs kinds`
//! contract: walks docs/kinds/
//! renders `docs/kinds/python.md`'s `kind-fields` and `kind-ceilings`
//! blocks from the registry.

use std::path::Path;
use std::process::Command;

pub fn copy_pages(to: &Path) {
    let dir = to.join("docs/kinds");
    std::fs::create_dir_all(&dir).unwrap();
    for entry in std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/kinds")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dir.join(entry.file_name())).unwrap();
    }
}

fn block<'a>(page: &'a str, name: &str) -> &'a str {
    let start = format!("<!-- {name}:start -->");
    let end = format!("<!-- {name}:end -->");
    let s = page.find(&start).unwrap_or_else(|| panic!("missing {start}")) + start.len();
    let e = page.find(&end).unwrap_or_else(|| panic!("missing {end}"));
    &page[s..e]
}

#[test]
fn kdr_ac02_python_page_tables_generated() {
    let tmp = std::env::temp_dir().join(format!("kdr_ac02_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    copy_pages(&tmp);
    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["gen-docs", "kinds"])
        .current_dir(&tmp)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let page = std::fs::read_to_string(tmp.join("docs/kinds/python.md")).unwrap();

    let registry = mcphost::kinds::production_registry();
    let python = registry.get("python").unwrap();
    let mut expected: Vec<String> = python.known_spec_fields().iter().map(|f| f.to_string()).collect();
    expected.extend(mcphost::kinds::UNIVERSAL_SPEC_FIELDS.iter().map(|f| f.to_string()));
    expected.sort();

    let mut listed = Vec::new();
    for row in block(&page, "kind-fields").lines().filter(|l| l.starts_with('|')).skip(2) {
        let cells: Vec<&str> = row.trim_matches('|').split(" | ").map(str::trim).collect();
        assert_eq!(cells.len(), 4, "row `{row}`");
        assert!(!cells[3].is_empty(), "field {} has no meaning", cells[0]);
        listed.push(cells[0].trim_matches('`').to_string());
    }
    listed.sort();
    assert_eq!(listed, expected);

    assert!(
        block(&page, "kind-ceilings").contains("| `budget.max_tool_latency_ms` | 30000 | 120000 |"),
        "kind-ceilings: {}",
        block(&page, "kind-ceilings")
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

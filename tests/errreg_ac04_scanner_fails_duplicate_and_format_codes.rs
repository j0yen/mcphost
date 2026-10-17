//! PRD-mcphost-error-codes-generated-registry
//! AC4 (P0) — Given a fixture tree where one code is declared by `err_code!`
//! at two sites or built with `format!` after one of the four prefixes, When
//! the scanner runs, Then it fails naming both sites for the duplicate and
//! the site for the `format!`.

use mcphost::error_scan::{ViolationKind, scan_tree};

/// A throwaway `<tmp>/src` tree, removed on drop.
struct Fixture(std::path::PathBuf);

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fixture(files: &[(&str, &str)]) -> Fixture {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("errreg-ac04-{}-{n}", std::process::id()));
    let src = root.join("src");
    std::fs::create_dir_all(&src).expect("mkdir");
    for (name, text) in files {
        std::fs::write(src.join(name), text).expect("write fixture");
    }
    Fixture(root)
}

fn scan(dir: &Fixture) -> Vec<mcphost::error_scan::Violation> {
    scan_tree(&dir.0.join("src")).expect("scan")
}

fn decl(name: &str, code: &str) -> String {
    format!(
        "err_code! {{\n    pub {name},\n    code: \"{code}\", family: Args, class: AgentFault, retryable: false,\n    remedy: \"Fix it.\",\n}}\n"
    )
}

#[test]
fn errreg_ac04_code_declared_at_two_sites_names_both() {
    let dir = fixture(&[
        ("a.rs", &decl("WIDGET_BROKEN", "widget_broken")),
        ("b.rs", &format!("fn pad() {{}}\n{}", decl("WIDGET_BROKEN_AGAIN", "widget_broken"))),
        ("c.rs", &decl("OTHER", "other_code")),
    ]);
    let found = scan(&dir);
    assert_eq!(found.len(), 1, "{found:?}");
    let v = &found[0];
    assert_eq!(v.kind, ViolationKind::Duplicate);
    assert_eq!(v.code, "widget_broken");
    let line = v.to_string();
    assert!(line.contains("src/b.rs:2"), "second site missing: {line}");
    assert!(line.contains("src/a.rs:1"), "first site missing: {line}");
}

#[test]
fn errreg_ac04_arrow_form_counts_as_a_declaration_for_duplicates() {
    let arrow = "err_code! { pub X => \"widget_broken\", Args, AgentFault, false, \"Fix it.\" }\n";
    let dir = fixture(&[("a.rs", &decl("WIDGET_BROKEN", "widget_broken")), ("b.rs", arrow)]);
    let found = scan(&dir);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].kind, ViolationKind::Duplicate);
}

#[test]
fn errreg_ac04_format_built_code_after_each_prefix_names_the_site() {
    let cases: [(&str, &str); 5] = [
        ("code:", "fn f() { E::S { code: format!(\"w_{}\", n), message: m }; }\n"),
        ("structured(", "fn f() {\n    structured(\n        format!(\"w_{n}\"), m);\n}\n"),
        ("structured_with(", "fn f() { structured_with(format!(\"w_{n}\"), m, d); }\n"),
        ("\"error_code\":", "fn f() { json!({ \"error_code\": format!(\"w_{n}\") }); }\n"),
        (
            "AppError::code() arm",
            "impl AppError {\n    pub fn code(&self) -> &'static str {\n        match self {\n            AppError::A => format!(\"w\"),\n        }\n    }\n}\n",
        ),
    ];
    for (context, text) in cases {
        let dir = fixture(&[("a.rs", text)]);
        let found = scan(&dir);
        assert_eq!(found.len(), 1, "{context}: {found:?}");
        let v = &found[0];
        assert_eq!(v.kind, ViolationKind::FormatBuilt);
        assert_eq!(v.context, context);
        let line = v.to_string();
        assert!(line.contains(&format!("src/a.rs:{}:", v.line)), "{line}");
        assert!(line.contains("format!"), "{line}");
    }
}

#[test]
fn errreg_ac04_distinct_codes_each_declared_once_pass() {
    let dir = fixture(&[("a.rs", &decl("A", "code_a")), ("b.rs", &decl("B", "code_b"))]);
    assert!(scan(&dir).is_empty());
}

//! PRD-mcphost-error-codes-generated-registry
//! AC3 (P0) — Given a fixture tree with a bare literal after `code:`,
//! `structured(`, `structured_with(`, `"error_code":` or in an
//! `AppError::code()` arm, When the scanner runs, Then it fails naming
//! file:line and the literal, and the same tree with the literal replaced by
//! an `err_code!` const passes.

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
    let root = std::env::temp_dir().join(format!("errreg-ac03-{}-{n}", std::process::id()));
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

const DECL: &str = "err_code! {\n    pub WIDGET_BROKEN,\n    code: \"widget_broken\", family: Args, class: AgentFault, retryable: false,\n    remedy: \"Fix the widget.\",\n}\n";

#[test]
fn errreg_ac03_each_bare_literal_shape_fails_naming_file_line_and_literal() {
    let cases: [(&str, &str, &str); 5] = [
        ("code:", "fn f() -> E { E::S { code: \"widget_broken\", message: m } }\n", "widget_broken"),
        ("structured(", "fn f() {\n    let e = structured(\n        \"widget_broken\", m);\n}\n", "widget_broken"),
        ("structured_with(", "fn f() { let e = structured_with(\"widget_broken\", m, d); }\n", "widget_broken"),
        ("\"error_code\":", "fn f() { json!({ \"error_code\": \"widget_broken\" }); }\n", "widget_broken"),
        (
            "AppError::code() arm",
            "impl AppError {\n    pub fn code(&self) -> &'static str {\n        match self {\n            AppError::A => \"widget_broken\",\n        }\n    }\n}\n",
            "widget_broken",
        ),
    ];
    for (context, text, literal) in cases {
        let dir = fixture(&[("a.rs", text)]);
        let found = scan(&dir);
        assert_eq!(found.len(), 1, "{context}: {found:?}");
        let v = &found[0];
        assert_eq!(v.kind, ViolationKind::BareLiteral);
        assert_eq!(v.code, literal);
        assert_eq!(v.context, context);
        let line = v.to_string();
        assert!(line.contains(&format!("src/a.rs:{}:", v.line)), "{line}");
        assert!(line.contains(&format!("\"{literal}\"")), "{line}");
        assert!(line.contains("err_code!"), "R7 template missing: {line}");
    }
}

#[test]
fn errreg_ac03_literal_line_numbers_are_exact() {
    let dir = fixture(&[("a.rs", "fn a() {}\n\nfn f() { let e = structured(\"widget_broken\", m); }\n")]);
    let found = scan(&dir);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].line, 3);
}

#[test]
fn errreg_ac03_same_tree_with_err_code_consts_passes() {
    let text = format!(
        "{DECL}\nfn f() {{ let e = structured(WIDGET_BROKEN, m); }}\nfn g() -> E {{ E::S {{ code: WIDGET_BROKEN, message: m }} }}\nfn h() {{ json!({{ \"error_code\": WIDGET_BROKEN }}); }}\nfn i() {{ structured_with(WIDGET_BROKEN, m, d); }}\nimpl AppError {{\n    pub fn code(&self) -> &'static str {{\n        match self {{\n            AppError::A => WIDGET_BROKEN,\n        }}\n    }}\n}}\n// structured(\"widget_broken\") in a comment is not a raise site\n"
    );
    let dir = fixture(&[("a.rs", &text)]);
    let found = scan(&dir);
    assert!(found.is_empty(), "{found:?}");
}

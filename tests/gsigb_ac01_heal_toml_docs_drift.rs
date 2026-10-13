//! PRD-mcphost-golden-signals-self-probe-b AC1 (P0): `Signal::ALL`, the
//! `heal.toml` `[signals.*]` keys and the rows of the signals table in
//! `docs/self-healing.md` are the same set, and adding `[signals.extra]` to
//! the toml fails.

use std::collections::BTreeSet;
use std::process::Command;

use mcphost::signals::{HealConfig, Signal};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn all_names() -> BTreeSet<String> {
    Signal::ALL.iter().map(|s| s.name().to_string()).collect()
}

fn toml_keys(text: &str) -> BTreeSet<String> {
    let v: toml::Value = toml::from_str(text).expect("heal.toml parses");
    v["signals"].as_table().expect("[signals] table").keys().cloned().collect()
}

fn doc_rows(text: &str) -> BTreeSet<String> {
    let start = text.find("<!-- mcphost-heal-signals:start -->").expect("start marker");
    let end = text.find("<!-- mcphost-heal-signals:end -->").expect("end marker");
    text[start..end]
        .lines()
        .filter_map(|l| l.strip_prefix("| `"))
        .filter_map(|l| l.split('`').next())
        .map(str::to_string)
        .collect()
}

fn run_check(toml_path: &std::path::Path) -> std::process::Output {
    Command::new("bash")
        .arg(format!("{ROOT}/scripts/heal-doc-check.sh"))
        .env("HEAL_TOML", toml_path)
        .env("HEAL_DOC", format!("{ROOT}/docs/self-healing.md"))
        .output()
        .expect("run heal-doc-check.sh")
}

#[test]
fn toml_keys_and_docs_rows_equal_signal_all_and_an_extra_key_fails() {
    let toml_text = std::fs::read_to_string(format!("{ROOT}/heal.toml")).expect("read heal.toml");
    let doc_text = std::fs::read_to_string(format!("{ROOT}/docs/self-healing.md")).expect("read docs");
    assert_eq!(toml_keys(&toml_text), all_names(), "heal.toml keys drifted from Signal::ALL");
    assert_eq!(doc_rows(&doc_text), all_names(), "docs table rows drifted from Signal::ALL");

    let ok = run_check(std::path::Path::new(&format!("{ROOT}/heal.toml")));
    assert!(ok.status.success(), "stderr: {}", String::from_utf8_lossy(&ok.stderr));

    let extended = format!("{toml_text}\n[signals.extra]\namber = 1\nred = 2\nfor_s = 3\nunit = \"x\"\n");
    assert_ne!(toml_keys(&extended), all_names(), "the extra key is visible to the key comparison");
    assert!(HealConfig::parse(&extended).is_err(), "the boot parser rejects [signals.extra]");
    let dir = std::env::temp_dir().join(format!("gsigb_ac01_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let scratch = dir.join("heal.toml");
    std::fs::write(&scratch, extended).expect("write scratch toml");
    let bad = run_check(&scratch);
    assert!(!bad.status.success(), "docs check must fail with [signals.extra]");
    let _ = std::fs::remove_dir_all(&dir);
}

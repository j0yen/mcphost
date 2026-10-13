//! PRD-mcphost-heal-action-registry-and-ledger AC1 (P0): `HealAction::ALL`,
//! the names in `docs/self-healing.md`, the names `/status.json.heal.actions`
//! returns and the names the ledger accepts are one set; an extra variant
//! fails the docs leg.

use crate::common;

use std::collections::BTreeSet;
use std::process::Command;

use mcphost::heal::{HealAction, HealOutcome, HealRow};
use serde_json::{Value, json};

fn all_names() -> BTreeSet<String> {
    HealAction::ALL.iter().map(|a| a.as_str().to_string()).collect()
}

fn docs_names() -> BTreeSet<String> {
    let docs = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/self-healing.md")).expect("read docs");
    // The page also holds the golden-signals table, so read only the `## Self-healing actions` section.
    let mut in_section = false;
    docs.lines()
        .filter(|l| {
            if l.starts_with("## ") {
                in_section = *l == "## Self-healing actions";
            }
            in_section
        })
        .filter_map(|l| l.strip_prefix("| `"))
        .filter_map(|l| l.split_once("` |").map(|(n, _)| n.to_string()))
        .collect()
}

#[tokio::test]
async fn heal_action_set_is_one_set() {
    let all = all_names();
    assert_eq!(all.len(), HealAction::ALL.len(), "names are unique");

    assert_eq!(docs_names(), all, "docs leg");

    let (state, _dir) = common::bare_app_state().await;
    let status = mcphost::statusfeed::status_json(&state).await.expect("status_json");
    let served: BTreeSet<String> = status["heal"]["actions"]
        .as_array()
        .expect("heal.actions")
        .iter()
        .map(|v| v.as_str().expect("name").to_string())
        .collect();
    assert_eq!(served, all, "status.json leg");

    for a in HealAction::ALL {
        let id = state
            .db
            .insert_heal_row(HealRow {
                id: 0,
                action: *a,
                at: 1,
                outcome: HealOutcome::Ok,
                dry_run: false,
                duration_ms: 0,
                trigger: json!({}),
                before: Value::Null,
                after: Value::Null,
            })
            .await
            .expect("ledger accepts every HealAction");
        assert!(id > 0);
    }
    let accepted: BTreeSet<String> = all.iter().filter(|n| n.parse::<HealAction>().is_ok()).cloned().collect();
    assert_eq!(accepted, all, "ledger leg");
    assert!("bogus_action".parse::<HealAction>().is_err());
}

#[test]
fn heal_extra_variant_fails_docs_leg() {
    let root = env!("CARGO_MANIFEST_DIR");
    let ok = Command::new(format!("{root}/scripts/heal-doc-check.sh")).output().expect("run script");
    assert!(ok.status.success(), "unmodified source is in sync: {ok:?}");

    let src = std::fs::read_to_string(format!("{root}/src/heal.rs")).expect("read heal.rs");
    let patched = src.replacen(
        "    LogRotate => (",
        "    ExtraVariant => (\"extra_variant\", 1, \"Added by the drift test.\"),\n    LogRotate => (",
        1,
    );
    assert_ne!(patched, src, "patch applied");
    let tmp = std::env::temp_dir().join(format!("heal_ac01_{}.rs", std::process::id()));
    std::fs::write(&tmp, patched).expect("write patched");
    let bad = Command::new(format!("{root}/scripts/heal-doc-check.sh"))
        .arg("--source")
        .arg(&tmp)
        .output()
        .expect("run script");
    let _ = std::fs::remove_file(&tmp);
    assert_eq!(bad.status.code(), Some(1), "extra variant fails: {bad:?}");
    assert!(String::from_utf8_lossy(&bad.stderr).contains("extra_variant"));
}

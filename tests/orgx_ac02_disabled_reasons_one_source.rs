//! contract: src/ writes tenants.disabled_reason only through auth_shape::DISABLED_REASONS
//! PRD-mcphost-org-export-and-offboard-v2
//! AC2 — Given `DISABLED_REASONS` in `src/auth_shape.rs`, When the drift test
//! scans `src/` for `disabled_reason` writes and calls `admin.tenants`
//! with each value and with `bogus`, Then no write spells a literal outside
//! the const, each value filters its tenants, `bogus` is `args_invalid`, and
//! `SYNTHETIC_PURGE_REASONS` still equals `[idle_expired, self_offboard]`.

use std::path::{Path, PathBuf};

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use mcphost::auth_shape::DISABLED_REASONS;
use serde_json::json;

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read src dir").flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// `src/` lines that write a `disabled_reason` with a bare literal, or spell a
/// `DISABLED_REASONS` value as a string literal anywhere outside `auth_shape.rs`.
fn literal_writes() -> Vec<String> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    let sql_literal = regex::Regex::new(r"disabled_reason\s*=\s*'[a-z_]+'").expect("regex");
    let some_literal = regex::Regex::new(r#"disabled_reason:\s*Some\(\s*""#).expect("regex");
    let mut found = Vec::new();
    for file in files {
        let rel = file.strip_prefix(&src).expect("under src").to_string_lossy().to_string();
        let text = std::fs::read_to_string(&file).expect("read source");
        for (i, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            let spells_value = rel != "auth_shape.rs"
                && DISABLED_REASONS.iter().any(|r| line.contains(&format!("\"{r}\"")) || line.contains(&format!("'{r}'")));
            if sql_literal.is_match(line) || some_literal.is_match(line) || spells_value {
                found.push(format!("src/{rel}:{}: {}", i + 1, line.trim()));
            }
        }
    }
    found
}

#[tokio::test]
async fn orgx_ac02() {
    assert_eq!(
        DISABLED_REASONS,
        &["self_offboard", "admin_disable", "idle_expired", "unclaimed_expiring", "unclaimed_expired", "org_offboard"]
    );
    let writes = literal_writes();
    assert!(writes.is_empty(), "disabled_reason written with a literal outside auth_shape::DISABLED_REASONS:\n{}", writes.join("\n"));
    assert_eq!(mcphost::retention::SYNTHETIC_PURGE_REASONS, &["idle_expired", "self_offboard"]);

    // One tenant per reason, plus an enabled one that no filter may return.
    let server = TestServer::start_with_signup_rate_limit(100).await;
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let mut by_reason = Vec::new();
    for reason in DISABLED_REASONS {
        let (ns, _) = signup(&server.base_url, &format!("tenant {reason}")).await;
        conn.execute("UPDATE tenants SET disabled = 1, disabled_reason = ?2 WHERE namespace = ?1", rusqlite::params![ns, reason])
            .expect("disable");
        by_reason.push((*reason, ns));
    }
    let (enabled_ns, _) = signup(&server.base_url, "enabled tenant").await;

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    for (reason, ns) in &by_reason {
        let out = extract_structured(&admin.tools_call("admin.tenants", json!({"disabled_reason": reason})).await.expect("tenants"));
        let names: Vec<&str> = out["tenants"].as_array().expect("tenants").iter().map(|t| t["tenant"].as_str().unwrap()).collect();
        assert_eq!(names, vec![ns.as_str()], "disabled_reason={reason}: {out}");
        assert!(!names.contains(&enabled_ns.as_str()));
    }
    let err = admin.tools_call("admin.tenants", json!({"disabled_reason": "bogus"})).await.expect_err("bogus");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
}

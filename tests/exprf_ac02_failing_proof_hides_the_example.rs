//! PRD-mcphost-example-proof-nightly-and-hide
//! AC2 — Given `TestServer` and a stub proof forced to fail, When the runner
//! posts with `--report`, Then `example_proofs` holds `ok: false`,
//! `hidden_reason` set, `red_since` set, and `host.example.list` omits the
//! name while `host.example.fork` returns `example_hidden` with the reason.

use crate::exprf;

use crate::common;
use common::{ADMIN_KEY, extract_structured};
use mcphost::example_proofs::{RunOpts, run};
use serde_json::json;

#[tokio::test]
async fn exprf_ac02() {
    let f = exprf::start().await;
    let victim = exprf::names()[0];
    let files_dir = exprf::stub_proofs(&[victim], None);
    assert!(exprf::listed_names(&f.tenant).await.contains(&victim.to_string()), "visible before any report");

    let opts = RunOpts {
        url: f.server.base_url.clone(),
        only: None,
        report: true,
        dry_run: false,
        admin_key: Some(ADMIN_KEY.to_string()),
        files_dir: Some(files_dir),
    };
    let mut out = Vec::new();
    let (runs, all_ok) = run(&opts, &mut out).await.expect("runner");
    assert!(!all_ok, "one proof failed, so the runner reports failure");
    assert_eq!(runs.len(), exprf::names().len());
    let text = String::from_utf8_lossy(&out);
    assert!(text.contains(&format!("{victim} FAIL")), "one line per example: {text}");

    // The row, read back through the admin-visible list tool.
    let db_row = f.server.state.db.get_example_proof(victim.to_string()).await.expect("db").expect("row");
    assert!(!db_row.ok);
    assert!(db_row.hidden_reason.as_deref().is_some_and(|r| r.starts_with("proof failed ")), "{db_row:?}");
    assert!(db_row.red_since.is_some(), "{db_row:?}");
    assert!(db_row.log_excerpt.contains("stub proof"), "excerpt: {}", db_row.log_excerpt);

    let names = exprf::listed_names(&f.tenant).await;
    assert!(!names.contains(&victim.to_string()), "hidden example still listed: {names:?}");
    assert_eq!(names.len(), exprf::names().len() - 1);

    let err = f.tenant.tools_call("host.example.fork", json!({"name": victim})).await.expect_err("hidden fork refused");
    assert_eq!(err.error_code.as_deref(), Some("example_hidden"), "{} {}", err.code, err.message);
    assert!(err.message.contains("proof failed"), "reason in the message: {}", err.message);

    // A passing example still forks.
    let other = exprf::names()[1];
    let ok = extract_structured(&f.tenant.tools_call("host.example.fork", json!({"name": other})).await.expect("visible fork"));
    assert_eq!(ok["name"], other);
}

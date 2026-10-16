//! PRD-mcphost-example-proof-nightly-and-hide
//! AC7 — Given the runner with `--only uptime-probes --dry-run`, When it
//! runs, Then it prints that one name and executes nothing.

use crate::exprf;

use mcphost::example_proofs::{RunOpts, run};

#[tokio::test]
async fn exprf_ac07() {
    assert!(exprf::names().contains(&"uptime-probes"), "the embedded index names uptime-probes");
    let marker = std::env::temp_dir().join(format!("exprf-ac07-marker-{}-{}", std::process::id(), mcphost::state::now_unix_ms()));
    let files_dir = exprf::stub_proofs(&[], Some(&marker));

    let opts = RunOpts {
        url: "http://127.0.0.1:1/mcp".to_string(),
        only: Some("uptime-probes".to_string()),
        report: false,
        dry_run: true,
        admin_key: None,
        files_dir: Some(files_dir.clone()),
    };
    let mut out = Vec::new();
    let (runs, all_ok) = run(&opts, &mut out).await.expect("dry run");
    assert_eq!(String::from_utf8_lossy(&out).trim(), "uptime-probes", "prints exactly that one name");
    assert!(runs.is_empty() && all_ok, "nothing executed");
    assert!(!marker.exists(), "no proof script ran");

    // Control: without --dry-run the same stub does run, so the marker proves something.
    let live = RunOpts { dry_run: false, ..opts };
    let (runs, _) = run(&live, &mut Vec::new()).await.expect("real run");
    assert_eq!(runs.len(), 1);
    assert!(marker.exists(), "the stub touches the marker when it executes");
    let _ = std::fs::remove_file(&marker);
}

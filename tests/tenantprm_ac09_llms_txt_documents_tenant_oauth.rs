//! PRD-mcphost-tenant-resource-metadata
//! AC9 (P1) — Given `www/llms.txt`, When read, Then a section names the
//! per-tenant URI, the audience rule, and that `/mcp` stays key-based, and
//! `scripts/gen-docs-sharing.sh --check` exits 0.

use std::path::Path;
use std::process::Command;

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn llms_txt() -> String {
    std::fs::read_to_string(repo_root().join("www/llms.txt")).expect("read www/llms.txt")
}

#[test]
fn section_names_the_per_tenant_uri_the_audience_rule_and_that_mcp_stays_key_based() {
    let text = llms_txt();
    let start = text
        .find("## Connect an OAuth client to your tenant")
        .expect("www/llms.txt must have a 'Connect an OAuth client to your tenant' section");
    let end = text[start..]
        .find("<!-- sharing:start -->")
        .map(|i| start + i)
        .unwrap_or(text.len());
    let section = &text[start..end];

    assert!(
        section.contains("/t/<your-namespace>/mcp") || section.contains("/t/{ns}/mcp"),
        "section must name the per-tenant resource URI: {section}"
    );
    assert!(
        section.to_lowercase().contains("audience"),
        "section must name the audience rule: {section}"
    );
    assert!(
        section.contains("/mcp") && section.to_lowercase().contains("key-based"),
        "section must say /mcp stays key-based: {section}"
    );
}

/// Same "run the real script against a scratch copy" convention as
/// `sharedcall_ac05_gen_docs_sharing_check.rs`'s own `check_passes_on_the_committed_files`
/// -- this PRD's new section lives above `<!-- sharing:start -->`, entirely
/// outside the marked block `gen-docs-sharing.sh` regenerates, so it must
/// still exit 0 unchanged.
#[test]
fn gen_docs_sharing_check_still_exits_zero() {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-tenantprm-ac09-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    for sub in ["scripts", "docs", "www"] {
        std::fs::create_dir_all(dir.join(sub)).expect("create scratch subdir");
    }
    for rel in ["scripts/gen-docs-sharing.sh", "docs/sharing.md", "www/llms.txt"] {
        std::fs::copy(repo_root().join(rel), dir.join(rel)).unwrap_or_else(|e| panic!("copy {rel}: {e}"));
    }

    let output = Command::new("bash")
        .arg(dir.join("scripts/gen-docs-sharing.sh"))
        .arg("--check")
        .output()
        .expect("run gen-docs-sharing.sh --check");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "gen-docs-sharing.sh --check must exit 0 with this PRD's section added above sharing:start, stderr:\n{stderr}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

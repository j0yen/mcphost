//! PRD-mcphost-upstream-token-vault-status
//! AC8 (P1) — Given `www/llms.txt`, When read, Then an "Upstream token
//! vault" section above `<!-- sharing:start -->` names
//! `host.vault.provider_set` (with the three presets),
//! `host.vault.connect_link`, `host.vault.status`, `host.vault.disconnect`,
//! `host.vault.provider_remove`, the `upstream:` tool-spec field, and
//! `upstream_not_connected`, and `scripts/gen-docs-sharing.sh --check`
//! exits 0.

use std::path::Path;
use std::process::Command;

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn llms_txt_upstream_token_vault_section_names_every_tool_and_preset() {
    let llms = std::fs::read_to_string(repo_root().join("www/llms.txt")).expect("read www/llms.txt");
    let section_start = llms
        .find("## Upstream token vault")
        .expect("www/llms.txt must have an 'Upstream token vault' section");
    let sharing_start = llms
        .find("<!-- sharing:start -->")
        .expect("www/llms.txt must still have the sharing:start marker");
    assert!(
        section_start < sharing_start,
        "the vault section (at {section_start}) must be placed above sharing:start (at {sharing_start})"
    );

    let section = &llms[section_start..sharing_start];
    for needle in [
        "host.vault.provider_set",
        "host.vault.connect_link",
        "host.vault.status",
        "host.vault.disconnect",
        "host.vault.provider_remove",
        "upstream:",
        "upstream_not_connected",
        "slack",
        "github",
        "google",
    ] {
        assert!(section.contains(needle), "vault section must name '{needle}':\n{section}");
    }
}

/// Same scratch-copy technique as `sharedcall_ac05_gen_docs_sharing_check.rs`
/// (never the repo's own on-disk `www/llms.txt`, so a concurrently-running
/// test can't race this one over the same file).
fn scratch_copy(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-vaultst-ac08-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    for sub in ["scripts", "docs", "www"] {
        std::fs::create_dir_all(dir.join(sub)).expect("create scratch subdir");
    }
    for rel in ["scripts/gen-docs-sharing.sh", "docs/sharing.md", "www/llms.txt"] {
        std::fs::copy(repo_root().join(rel), dir.join(rel))
            .unwrap_or_else(|e| panic!("copy {rel} into scratch: {e}"));
    }
    dir
}

#[test]
fn gen_docs_sharing_check_stays_green() {
    let dir = scratch_copy("check");
    let output = Command::new("bash")
        .arg(dir.join("scripts/gen-docs-sharing.sh"))
        .arg("--check")
        .output()
        .expect("run gen-docs-sharing.sh --check");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "gen-docs-sharing.sh --check must exit 0, stderr:\n{stderr}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

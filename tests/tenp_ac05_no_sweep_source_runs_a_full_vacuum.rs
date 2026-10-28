//! PRD-mcphost-synthetic-tenant-purge
//! AC5 (P0) — each purge batch reclaims pages with the bounded incremental_vacuum, never a full
//! VACUUM. Kept apart from `tenp_ac05_purge_waits_for_grace_reports_first_and_reclaims_pages_in_bounds.rs`
//! so this source contract stays out of the env-guarded sibling suite file.
//! contract: the purge's page reclaim is the bounded incremental_vacuum and no sweep source runs a full VACUUM

#[test]
fn tenp_ac05_no_sweep_source_runs_a_full_vacuum() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for file in ["retention.rs", "db/tenant_purge.rs"] {
        let text = std::fs::read_to_string(root.join(file)).unwrap();
        let code: Vec<&str> = text.lines().filter(|l| !l.trim_start().starts_with("//")).collect();
        let joined = code.join("\n").replace("PURGE_VACUUM_PAGES", "");
        assert!(!joined.contains("VACUUM"), "{file}: a full VACUUM would be spelled in capitals");
        assert!(code.iter().any(|l| l.contains("incremental_vacuum(") && l.contains("PURGE_VACUUM_PAGES")), "{file} reclaims with incremental_vacuum(PURGE_VACUUM_PAGES)");
    }
}

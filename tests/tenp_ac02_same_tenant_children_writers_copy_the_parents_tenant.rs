//! PRD-mcphost-synthetic-tenant-purge
//! AC2 (P0) — the same-tenant children of the cascade (notify_receipts, oauth_jti_denylist) are
//! cleared by their parent's cascade only because their one writer copies the parent's tenant.
//! Kept apart from `tenp_ac02_cascade_complete_and_entangled_subject_held.rs` so this source
//! contract stays out of the env-guarded sibling suite file.
//! contract: the only writers of the same-tenant children copy the parent's tenant

#[test]
fn tenp_ac02_same_tenant_children_are_written_by_the_only_insert_that_copies_the_parents_tenant() {
    fn sources(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push((path.display().to_string(), std::fs::read_to_string(&path).unwrap()));
            }
        }
    }
    let mut files = Vec::new();
    sources(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
    let count = |needle: &str| -> Vec<&str> {
        files.iter().filter(|(_, text)| text.contains(needle)).map(|(path, _)| path.as_str()).collect()
    };
    // notify_receipts <- notifies: one INSERT per table, both binding the same `tenant_id`.
    assert_eq!(count("INSERT INTO notify_receipts").len(), 1, "one writer of notify_receipts");
    assert_eq!(count("INSERT INTO notifies").len(), 1, "one writer of notifies");
    let notify = &files.iter().find(|(p, _)| p.ends_with("src/notify.rs")).unwrap().1;
    assert!(notify.contains("params![nid, tenant_id, row_text, level.as_str(), row_link, now]"));
    assert!(notify.contains("params![nid, tenant_id, ch.id, ch.kind.as_str(), now]"), "the receipt copies the notify's tenant");
    // oauth_jti_denylist <- oauth_grants: one INSERT, one caller, the caller passes the grant's tenant.
    assert_eq!(count("INSERT INTO oauth_jti_denylist").len(), 1, "one writer of oauth_jti_denylist");
    assert_eq!(count(".insert_oauth_jti(").len(), 1, "one caller");
    let authz = &files.iter().find(|(p, _)| p.ends_with("src/authz.rs")).unwrap().1;
    assert!(authz.contains(".insert_oauth_jti(jti, grant_id, tenant_id, resource.to_string(), exp)"));
}

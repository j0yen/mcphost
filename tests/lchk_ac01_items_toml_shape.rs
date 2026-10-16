//! PRD-mcphost-launch-check-generated AC1 (P0) -- Given the initial
//! `launch/items.toml`, When counted, Then at least 8 `host` rows exist each
//! with a `probe` that contains no credential-shaped token and an `expect`,
//! and every `joe` row has a `doc` and a `url`.

use mcphost::launch_check::{Owner, load_items_file};

#[test]
fn lchk_ac01_initial_items_have_probes_without_credentials_and_joe_rows_have_doc_and_url() {
    let root = env!("CARGO_MANIFEST_DIR");
    let items = load_items_file(&std::path::Path::new(root).join("launch/items.toml")).unwrap();
    let hosts: Vec<_> = items.iter().filter(|i| i.owner == Owner::Host).collect();
    assert!(hosts.len() >= 8, "only {} host rows", hosts.len());
    for h in &hosts {
        let probe = h.probe.as_deref().unwrap_or_default();
        assert!(!probe.trim().is_empty(), "{} has no probe", h.id);
        let lower = probe.to_lowercase();
        for bad in ["key", "token", "secret", "bearer"] {
            assert!(!lower.contains(bad), "{} probe contains `{bad}`: {probe}", h.id);
        }
        assert!(!h.expect.trim().is_empty(), "{} has no expect", h.id);
    }
    let joes: Vec<_> = items.iter().filter(|i| i.owner == Owner::Joe).collect();
    assert!(!joes.is_empty());
    for j in joes {
        assert!(!j.doc.trim().is_empty(), "{} has no doc", j.id);
        assert!(j.url.as_deref().is_some_and(|u| !u.trim().is_empty()), "{} has no url", j.id);
    }
}

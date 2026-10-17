//! PRD-mcphost-launch-check-generated-b AC1 (P0) -- Given `README.md` with N
//! distinct external link hosts, When rendered, Then exactly N
//! `readme-link-*` rows exist and the test fails if one is added to the
//! README without appearing.

use mcphost::launch_check::{effective_items, load_items_file, readme_external_links, render_checklist};

fn distinct_hosts(readme: &str) -> Vec<String> {
    let mut hosts: Vec<String> = readme_external_links(readme)
        .iter()
        .map(|u| u.split("://").nth(1).unwrap().split(['/', ':']).next().unwrap().to_ascii_lowercase())
        .collect();
    hosts.sort();
    hosts.dedup();
    hosts
}

#[test]
fn lchkb_ac01_rendered_readme_link_rows_equal_distinct_hosts() {
    let root = env!("CARGO_MANIFEST_DIR");
    let readme = std::fs::read_to_string(format!("{root}/README.md")).unwrap();
    let items = load_items_file(&std::path::Path::new(root).join("launch/items.toml")).unwrap();
    let md = render_checklist(&effective_items(items, Some(&readme)));
    let n = distinct_hosts(&readme).len();
    assert!(n > 0, "README has no external links");
    assert_eq!(md.matches("| readme-link-").count(), n, "{md}");
    // The committed checklist must carry the same rows: a link added to the
    // README without a re-render fails here.
    let committed = std::fs::read_to_string(format!("{root}/docs/launch/wave1-checklist.md")).unwrap();
    assert_eq!(committed.matches("| readme-link-").count(), n, "README link set drifted from the committed checklist");
}

#[test]
fn lchkb_ac01_a_new_readme_host_adds_exactly_one_row() {
    let base = "See https://alpha.example.org/a and https://beta.example.net/b.\n";
    let more = format!("{base}Also https://gamma.example.io/c and https://alpha.example.org/again.\n");
    let count = |r: &str| render_checklist(&effective_items(vec![], Some(r))).matches("| readme-link-").count();
    assert_eq!(count(base), 2);
    assert_eq!(count(&more), 3);
}

//! AC7 (hotfix install-doc-copilot-sdk) -- `www/install-surfaces.json` is the
//! install surface list the discovery chain's install-walk reads instead of
//! keeping its own copy: the committed file equals the in-binary
//! `SURFACE_IDS` (one source of truth), and `mcphost gen-docs` writes it.

use mcphost::install_links::{self, SURFACE_IDS};

const COMMITTED: &str = include_str!("../www/install-surfaces.json");

#[test]
fn committed_json_lists_exactly_the_in_binary_surface_ids_in_order() {
    let v: serde_json::Value = serde_json::from_str(COMMITTED).expect("www/install-surfaces.json parses");
    let listed: Vec<&str> = v["surfaces"]
        .as_array()
        .expect("`surfaces` is an array")
        .iter()
        .map(|s| s.as_str().expect("surface ids are strings"))
        .collect();
    assert_eq!(listed, SURFACE_IDS.to_vec(), "run `mcphost gen-docs`");
    assert_eq!(v.as_object().map(|o| o.len()), Some(1), "only the `surfaces` key");
}

#[test]
fn committed_json_is_byte_identical_to_the_render() {
    assert_eq!(COMMITTED, install_links::render_surfaces_json(), "run `mcphost gen-docs`");
}

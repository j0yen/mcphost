//! AC3 (PRD-mcphost-chatgpt-submission-pack) — Given `metadata.json`,
//! `registry/server.json`, and the landing page, When their descriptions are
//! compared, Then all three are identical.

use mcphost::submission_pack;

fn root(path: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path)
}

fn json_file(path: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))).expect("json")
}

#[test]
fn pack_server_json_and_landing_page_share_one_description() {
    let pack = json_file(&root(submission_pack::PACK_PATH).join("metadata.json"));
    let server = json_file(&root("registry/server.json"));
    let landing = std::fs::read_to_string(root("www/index.html")).expect("www/index.html");
    let meta = submission_pack::landing_meta_description(&landing).expect("landing meta description");

    let pack_description = pack["short_description"].as_str().expect("short_description");
    let server_description = server["description"].as_str().expect("server.json description");
    assert_eq!(pack_description, server_description, "metadata.json vs registry/server.json");
    assert_eq!(pack_description, meta, "metadata.json vs landing page meta description");
    assert_eq!(pack_description, submission_pack::shared_description(), "all three come from the one source");
    assert!(pack_description.chars().count() <= submission_pack::MAX_SHORT_DESCRIPTION_CHARS);

    // The og: description is the same sentence, so link previews cannot drift either.
    assert!(
        landing.contains(&format!("<meta property=\"og:description\" content=\"{pack_description}\">")),
        "og:description must equal the shared description"
    );
}

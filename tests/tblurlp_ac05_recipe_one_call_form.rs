//! PRD-mcphost-table-import-url-live-proof
//! AC5 (P1) -- Given the hand-written `## Give Claude a database in one
//! minute` section of `www/llms.txt` (the generated
//! `recipe:database-in-a-minute` block stays generator-owned), When an agent
//! reads it, Then the one-call form (`host_table_create` with `source_url`
//! `https://mcphost.dev/examples/sample-1000.csv`) comes before the
//! batched-append step, and the AC 2 drift test covers that URL.
//!
//! The section spells the verb `host.table.create` (dots), as the rest of
//! `www/llms.txt` does; the client-visible form is the same verb with `.` -> `_`.

use mcphost::http::routes::{ROUTES, UNSPECCED};

const LLMS_TXT: &str = include_str!("../www/llms.txt");
const HEADING: &str = "\n## Give Claude a database in one minute\n";
const NEXT_HEADING: &str = "\n## Chart in a minute\n";
const FIXTURE_URL: &str = "https://mcphost.dev/examples/sample-1000.csv";

fn section() -> &'static str {
    let start = LLMS_TXT.find(HEADING).expect("the hand-written section heading") + 1;
    let end = LLMS_TXT[start..].find(NEXT_HEADING).map_or(LLMS_TXT.len(), |i| start + i);
    &LLMS_TXT[start..end]
}

#[test]
fn tblurlp_ac05_one_call_form_comes_before_the_batched_append() {
    let section = section();
    let one_call = section
        .lines()
        .position(|l| l.contains("host.table.create") && l.contains("source_url") && l.contains(FIXTURE_URL))
        .expect("a host.table.create call carrying source_url = the fixture URL");
    let append = section
        .lines()
        .position(|l| l.contains("host.table.append"))
        .expect("the batched-append step is still shown");
    assert!(one_call < append, "one-call form (line {one_call}) must precede the append step (line {append})");
    let batches = section.lines().position(|l| l.contains("batches of 200")).expect("batch-size sentence");
    assert!(one_call < batches, "one-call form must precede the batches-of-200 step");
}

#[test]
fn tblurlp_ac05_the_generated_recipe_block_is_untouched_and_the_url_has_a_route() {
    assert!(LLMS_TXT.contains("<!-- recipe:database-in-a-minute begin -->"));
    assert!(LLMS_TXT.contains("<!-- recipe:database-in-a-minute end -->"));
    let generated = &LLMS_TXT[LLMS_TXT.find("<!-- recipe:database-in-a-minute begin -->").unwrap()
        ..LLMS_TXT.find("<!-- recipe:database-in-a-minute end -->").unwrap()];
    assert!(!generated.contains("source_url"), "the generated block is owned by recipes/database-in-a-minute.toml");

    // The AC 2 drift test scans www/ for this URL; the route it must resolve to:
    assert!(section().contains(FIXTURE_URL));
    let path = FIXTURE_URL.strip_prefix("https://mcphost.dev").unwrap();
    assert!(
        ROUTES.iter().map(|r| r.path).chain(UNSPECCED.iter().copied()).any(|p| p == path),
        "{path} must be a route in the binary's static-route table"
    );
}

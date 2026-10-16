//! AC1 (the falsifier): an agent holding only llms.txt's `## Examples`
//! section can map a use-case phrasing to the example that matches it and the
//! one call that forks it. A scripted tag-matching reader resolves the 8
//! phrasings in `tests/fixtures/exgal_phrasings.txt`; fewer than 7 resolving
//! is the falsifier firing and the failure lists the misses.

use std::collections::BTreeSet;
use std::path::Path;

use mcphost::examples::EXAMPLE_USE_CASES;
use mcphost::examples_page::{EXAMPLES_SECTION_END, EXAMPLES_SECTION_START, fork_snippet};

const STOPWORDS: &[&str] = &["a", "an", "the", "my", "for", "of", "to", "and", "in", "with", "from", "that", "is", "on", "its", "not", "one", "all", "no"];

fn stems(text: &str) -> BTreeSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty() && !STOPWORDS.contains(w))
        .map(|w| w.chars().take(4).collect())
        .collect()
}

struct Row {
    name: String,
    /// name, title and tags, matched as separate fields.
    fields: [String; 3],
    fork: String,
}

fn section_rows() -> Vec<Row> {
    let llms = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("www/llms.txt")).unwrap();
    let start = llms.find(EXAMPLES_SECTION_START).expect("llms.txt has the examples begin marker");
    let end = llms.find(EXAMPLES_SECTION_END).expect("llms.txt has the examples end marker");
    llms[start..end]
        .lines()
        .filter_map(|l| l.strip_prefix("- "))
        .map(|l| {
            let parts: Vec<&str> = l.split(" — ").collect();
            assert_eq!(parts.len(), 5, "row `{l}` is name — title — tags — secrets — fork");
            Row { name: parts[0].to_string(), fields: [parts[0].to_string(), parts[1].to_string(), parts[2].to_string()], fork: parts[4].trim_start_matches("fork: ").to_string() }
        })
        .collect()
}

/// The tag-matching reader: the row whose name/title/tags share the most
/// word stems with the phrasing, counted per field (a word in the name, the
/// title and the tags is three hits); a tie or no overlap resolves nothing.
fn resolve<'a>(rows: &'a [Row], phrasing: &str) -> Option<&'a Row> {
    let want = stems(phrasing);
    let scored: Vec<(usize, &Row)> = rows.iter().map(|r| (r.fields.iter().map(|f| stems(f).intersection(&want).count()).sum::<usize>(), r)).collect();
    let best = scored.iter().map(|(s, _)| *s).max()?;
    let top: Vec<_> = scored.iter().filter(|(s, _)| *s == best && best > 0).collect();
    if top.len() == 1 { Some(top[0].1) } else { None }
}

#[test]
fn exgal_ac01_eight_phrasings_resolve_to_a_name_and_its_fork_snippet() {
    let fixture = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/exgal_phrasings.txt")).unwrap();
    let cases: Vec<Vec<&str>> = fixture.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()).map(|l| l.split('\t').collect()).collect();
    let tags: BTreeSet<&str> = cases.iter().map(|c| c[0]).collect();
    assert_eq!(cases.len(), 8);
    assert_eq!(tags, EXAMPLE_USE_CASES.iter().copied().collect::<BTreeSet<_>>(), "one phrasing per use-case tag");

    let rows = section_rows();
    assert!(rows.len() >= 7, "the section lists {} examples", rows.len());
    let mut misses = Vec::new();
    for c in &cases {
        let (tag, phrasing, want) = (c[0], c[1], c[2]);
        match resolve(&rows, phrasing) {
            Some(r) if r.name == want && r.fork == fork_snippet(want) => {}
            Some(r) => misses.push(format!("[{tag}] \"{phrasing}\" -> {} (wanted {want})", r.name)),
            None => misses.push(format!("[{tag}] \"{phrasing}\" -> nothing (wanted {want})")),
        }
    }
    assert!(
        cases.len() - misses.len() >= 7,
        "FALSIFIER FIRED: {}/8 phrasings resolved from llms.txt alone; misses:\n{}",
        cases.len() - misses.len(),
        misses.join("\n")
    );
}

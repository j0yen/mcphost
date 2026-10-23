//! PRD-mcphost-output-envelope-fields-generated AC2 -- Given `src/handler.rs`
//! and `src/calltrace.rs`, When a source scan matches `.insert("<lit>"`, Then
//! no literal is a member of `OUTPUT_ENVELOPE_FIELDS`, and every member is a
//! const in `src/envelope.rs`, the module `OUTPUT_ENVELOPE_FIELDS` is built in.

use mcphost::envelope::OUTPUT_ENVELOPE_FIELDS;
use regex::Regex;

/// `file:line` of every `.insert("<member>"` in `source`.
fn literal_member_inserts(file: &str, source: &str) -> Vec<String> {
    let re = Regex::new(r#"\.insert\(\s*"([^"]+)"\.to_string\(\)|\.insert\(\s*"([^"]+)""#).unwrap();
    let mut hits = Vec::new();
    for (i, line) in source.lines().enumerate() {
        for cap in re.captures_iter(line) {
            let lit = cap.get(1).or_else(|| cap.get(2)).unwrap().as_str();
            if OUTPUT_ENVELOPE_FIELDS.contains(&lit) {
                hits.push(format!("{file}:{}: insert of envelope member `{lit}` must use its const from src/envelope.rs", i + 1));
            }
        }
    }
    hits
}

#[test]
fn mcphost_output_envelope_fields_generated_ac02() {
    let root = env!("CARGO_MANIFEST_DIR");
    let mut hits = Vec::new();
    for file in ["src/handler.rs", "src/calltrace.rs"] {
        let source = std::fs::read_to_string(format!("{root}/{file}")).unwrap();
        hits.extend(literal_member_inserts(file, &source));
    }
    assert!(hits.is_empty(), "{}", hits.join("\n"));

    // The scan itself bites: a literal member insert is reported.
    let probe = literal_member_inserts("probe.rs", "obj.insert(\"call_id\".to_string(), v);");
    assert_eq!(probe.len(), 1, "the scan must flag a literal member insert");

    // Every member is a `pub const NAME: &str = "<member>";` in envelope.rs,
    // and the set is assembled from consts, not literals.
    let envelope = std::fs::read_to_string(format!("{root}/src/envelope.rs")).unwrap();
    for member in OUTPUT_ENVELOPE_FIELDS {
        let decl = Regex::new(&format!(r#"pub const [A-Z_]+: &str = "{}";"#, regex::escape(member))).unwrap();
        assert!(decl.is_match(&envelope), "`{member}` must be a pub const in src/envelope.rs");
    }
    assert!(
        envelope.contains("pub const RESOURCES_USED_KEY: &str = \"__resources_used\";"),
        "RESOURCES_USED_KEY lives in the envelope module"
    );
    let assembly = envelope
        .split("pub const OUTPUT_ENVELOPE_FIELDS")
        .nth(1)
        .and_then(|rest| rest.split(';').next())
        .expect("OUTPUT_ENVELOPE_FIELDS definition");
    assert!(!assembly.contains('"'), "OUTPUT_ENVELOPE_FIELDS must be built from consts, not literals: {assembly}");
}

//! PRD-mcphost-chain-near-miss-mapping-refused AC2 (P0) -- Given the class in
//! § Grammar class, When the generated incident table (§ Incident spellings,
//! vendored sources) and a property test over generated inputs run, Then
//! every input is accepted with one meaning (today's `StepRef::parse` `$.`
//! paths only) or is `will_fail` with `did_you_mean` an accepted `$.` path or
//! null, and each row matches its verdict and canonical-or-null.
//!
//! Every case is one `host.tool_publish dry_run` of the 3-step fixture chain
//! `tests/fixtures/chainmap_steps_fixture.json` with the case's value as the
//! one `args` entry of step `from_step` (0-based).

use crate::common;
use common::{McpClient, TestServer, chain_and_http_kind_registry, extract_structured, publish, signup};
use mcphost::kinds::chain::{CONTEXT_KEYS, ContextRoot, step_ref_parses};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;

fn root_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn read_json(rel: &str) -> Value {
    let text = std::fs::read_to_string(root_dir().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {rel}: {e}"))
}

/// What one dry run said about the case's mapping.
#[derive(Debug)]
enum Outcome {
    /// No failure entry for the case's step.
    Accepted,
    /// The failure entry for the case's step.
    Refused(Value),
    /// `parse_steps` refused a `$.item` / `$.index` path outside `for_each`,
    /// unchanged by this PRD and run on the `$.` form only.
    ItemOutsideForEach,
}

struct Fixture {
    _server: TestServer,
    client: McpClient,
    tools: Vec<String>,
}

async fn fixture() -> Fixture {
    let f = read_json("tests/fixtures/chainmap_steps_fixture.json");
    let server = TestServer::start_with_kinds(chain_and_http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Chainmap AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let mut tools = Vec::new();
    for step in f["steps"].as_array().expect("fixture steps") {
        let name = step["tool"].as_str().expect("fixture tool").to_string();
        let outputs = step["outputs"].as_array().expect("fixture outputs");
        if outputs.is_empty() {
            publish(&client, &name, "echo", json!({"schema": {"type": "object"}})).await;
        } else {
            let spec = json!({"method": "GET", "url": "http://127.0.0.1:1/x", "args_schema": {"type": "object"}, "outputs": outputs});
            publish(&client, &name, "http", spec).await;
        }
        tools.push(name);
    }
    Fixture { _server: server, client, tools }
}

impl Fixture {
    async fn dry_run(&self, v: &Value, from_step: usize) -> Outcome {
        let steps: Vec<Value> = self
            .tools
            .iter()
            .enumerate()
            .map(|(i, t)| json!({"tool": t, "args": if i == from_step { json!({"v": v}) } else { json!({}) }}))
            .collect();
        let got = self
            .client
            .tools_call(
                "host.tool_publish",
                json!({"name": "probe", "kind": "chain", "spec": {"steps": steps}, "dry_run": true}),
            )
            .await;
        let dry = match got {
            Ok(r) => extract_structured(&r),
            Err(e) if e.error_code.as_deref() == Some("compose_item_outside_for_each") => {
                return Outcome::ItemOutsideForEach;
            }
            Err(e) => panic!("dry run of {v} from step {from_step}: {e:?}"),
        };
        let gate = dry["gates"]
            .as_array()
            .expect("gates")
            .iter()
            .find(|g| g["gate"] == json!("verdict"))
            .unwrap_or_else(|| panic!("verdict gate: {dry}"));
        let entry = gate["failures"]
            .as_array()
            .and_then(|f| f.iter().find(|e| e["step"] == json!(from_step + 1)))
            .cloned();
        match entry {
            Some(e) => Outcome::Refused(e),
            None => Outcome::Accepted,
        }
    }

    /// `path` is a `$.` path the checker accepts from `from_step`.
    async fn accepts(&self, path: &str, from_step: usize) -> bool {
        matches!(self.dry_run(&json!(path), from_step).await, Outcome::Accepted | Outcome::ItemOutsideForEach)
    }
}

// ---- the grammar, written out independently of the checker ----

fn roots() -> Vec<ContextRoot> {
    CONTEXT_KEYS.roots().to_vec()
}

fn root_of<'a>(name: &str, roots: &'a [ContextRoot]) -> Option<&'a ContextRoot> {
    roots.iter().find(|r| r.name == name)
}

#[derive(Debug, PartialEq)]
enum Class {
    Literal,
    /// A `$.<root>...` path.
    Canonical,
    /// `$.<not a root>...`
    BadRoot,
    /// Reference-shaped, not a path: the expected rewrite.
    NearMiss(Option<String>),
}

fn split_root(s: &str) -> (&str, &str) {
    s.split_at(s.find(['.', '[']).unwrap_or(s.len()))
}

/// The rewrite of a bare / dollar spelling, as § Grammar class defines it.
fn rewrite(raw: &str, roots: &[ContextRoot]) -> Option<String> {
    let body = raw.strip_prefix("$.").or_else(|| raw.strip_prefix('$')).unwrap_or(raw);
    let (name, tail) = split_root(body);
    let root = root_of(name, roots)?;
    let mut tail = tail.to_string();
    if root.indexed {
        if let Some(rest) = tail.strip_prefix('.') {
            let (digits, after) = split_root(rest);
            if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            tail = format!("[{digits}]{after}");
        } else if !tail.starts_with('[') {
            return None;
        }
        let after_index = &tail[tail.find(']').unwrap() + 1..];
        let mut out = format!("$.{name}{tail}");
        if let Some(seg) = after_index.strip_prefix('.')
            && split_root(seg).0 != "result"
        {
            out = format!("$.{name}{}.result{after_index}", &tail[..tail.find(']').unwrap() + 1]);
        }
        return Some(out);
    }
    if root.has_result {
        let seg = tail.strip_prefix('.')?;
        if split_root(seg).0 != "result" {
            return Some(format!("$.{name}.result{tail}"));
        }
    }
    Some(format!("$.{name}{tail}"))
}

fn classify_string(s: &str, wrapped: bool, roots: &[ContextRoot]) -> Class {
    if let Some(rest) = s.strip_prefix("$.") {
        return match root_of(split_root(rest).0, roots) {
            Some(_) if wrapped => Class::NearMiss(Some(s.to_string())),
            Some(_) => Class::Canonical,
            None if wrapped => Class::Literal,
            None => Class::BadRoot,
        };
    }
    let (dollar, body) = match s.strip_prefix('$') {
        Some(b) => (true, b),
        None => (false, s),
    };
    let (name, tail) = split_root(body);
    let Some(root) = root_of(name, roots) else {
        return Class::Literal;
    };
    let segs = tail.matches('.').count();
    let shaped = dollar || root.has_result || tail.starts_with('[') || segs >= 2;
    if shaped { Class::NearMiss(rewrite(s, roots)) } else { Class::Literal }
}

fn classify(v: &Value, roots: &[ContextRoot]) -> (Class, Option<String>) {
    match v {
        Value::String(s) => (classify_string(s, false, roots), Some(s.clone())),
        Value::Object(m) if m.len() == 1 => match m.values().next().and_then(Value::as_str) {
            Some(s) => (classify_string(s, true, roots), Some(s.to_string())),
            None => (Class::Literal, None),
        },
        _ => (Class::Literal, None),
    }
}

// ---- generated inputs ----

/// Every root x prefix x index form x 0-3 segments, as bare strings.
fn generated_strings() -> Vec<String> {
    let segs = ["result", "rows"];
    let mut tails: Vec<Vec<&str>> = vec![vec![]];
    for _ in 0..3 {
        let next: Vec<Vec<&str>> = tails
            .iter()
            .filter(|t| t.len() < 3)
            .flat_map(|t| segs.iter().map(move |s| [t.as_slice(), &[*s]].concat()))
            .collect();
        tails.extend(next);
    }
    tails.sort();
    tails.dedup();
    let mut names: Vec<(String, bool)> = roots().iter().map(|r| (r.name.to_string(), r.indexed)).collect();
    names.push(("nope".to_string(), false));
    let mut out = Vec::new();
    for (name, indexed) in &names {
        let heads: Vec<String> = if *indexed {
            vec![name.clone(), format!("{name}.0"), format!("{name}[0]"), format!("{name}.2"), format!("{name}[1]")]
        } else {
            vec![name.clone()]
        };
        for head in heads {
            for prefix in ["$.", "$", ""] {
                for tail in &tails {
                    let dotted: String = tail.iter().map(|s| format!(".{s}")).collect();
                    out.push(format!("{prefix}{head}{dotted}"));
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

#[test]
fn canonical_set_matches_the_committed_snapshot() {
    let mut snapshot = serde_json::Map::new();
    for this_step_no in [1usize, 3] {
        let accepted: Vec<String> = generated_strings().into_iter().filter(|s| step_ref_parses(s, this_step_no)).collect();
        snapshot.insert(this_step_no.to_string(), json!(accepted));
    }
    let snapshot = Value::Object(snapshot);
    let rel = "tests/fixtures/chainmap_canonical_set.json";
    if std::env::var_os("CHAINMAP_WRITE_SNAPSHOT").is_some() {
        std::fs::write(root_dir().join(rel), serde_json::to_string_pretty(&snapshot).unwrap() + "\n").unwrap();
    }
    assert_eq!(snapshot, read_json(rel), "StepRef::parse accepts a different set; review the diff");

    // The grammar layer: a `$.` path of a `result`-holding root parses exactly
    // when the grammar says it is canonical.
    let rs = roots();
    for s in generated_strings() {
        for this_step_no in [1usize, 3] {
            let expected = match classify_string(&s, false, &rs) {
                Class::Canonical => {
                    let root = root_of(split_root(&s[2..]).0, &rs).unwrap();
                    root.has_result
                        && (root.indexed || this_step_no >= 2)
                        && (!root.indexed || {
                            let after = &s[2 + root.name.len()..];
                            after.starts_with('[')
                        })
                }
                _ => false,
            };
            assert_eq!(step_ref_parses(&s, this_step_no), expected, "{s} from step {this_step_no}");
        }
    }
}

/// `path` is `$.steps[N]...` with `N` at or past the 0-based `from_step`.
fn is_forward(path: &str, from_step: usize) -> bool {
    path.strip_prefix("$.steps[")
        .and_then(|rest| rest.split_once(']'))
        .and_then(|(n, _)| n.parse::<usize>().ok())
        .is_some_and(|n| n >= from_step)
}

const NEAR_MISS_REASONS: [&str; 3] = ["not a path", "path inside a literal object", "unknown root"];

fn did_you_mean(entry: &Value) -> Option<String> {
    entry.get("did_you_mean").and_then(Value::as_str).map(str::to_string)
}

#[tokio::test]
async fn property_every_generated_input_is_accepted_or_refused_with_an_accepted_rewrite() {
    let fx = fixture().await;
    let rs = roots();
    let from_step = 2;
    let mut inputs: Vec<Value> = Vec::new();
    for s in generated_strings() {
        inputs.push(json!(s));
        inputs.push(json!({"k": s}));
    }
    let mut refused = 0;
    for v in inputs {
        let (class, inner) = classify(&v, &rs);
        let outcome = fx.dry_run(&v, from_step).await;
        let ctx = format!("{v} -> {class:?} -> {outcome:?}");
        match (&class, &outcome) {
            (Class::Literal, Outcome::Accepted) => {}
            (Class::Canonical, Outcome::Accepted | Outcome::ItemOutsideForEach) => {}
            (Class::Canonical, Outcome::Refused(e)) => {
                // Only today's path check may refuse a canonical path.
                assert!(
                    !NEAR_MISS_REASONS.contains(&e["reason"].as_str().unwrap_or("")),
                    "a canonical path is never a near-miss: {ctx}"
                );
                // The shipped missing-hop entry rebuilds the path without
                // re-checking the field: canonical, not necessarily accepted.
                if let Some(d) = did_you_mean(e) {
                    assert_eq!(classify_string(&d, false, &rs), Class::Canonical, "did_you_mean {d}: {ctx}");
                }
            }
            (Class::BadRoot, Outcome::Refused(e)) => {
                assert_eq!(e["reason"], json!("unknown root"), "{ctx}");
                assert!(did_you_mean(e).is_none(), "{ctx}");
                refused += 1;
            }
            (Class::NearMiss(expected), Outcome::Refused(e)) => {
                let wrapped = v.is_object();
                let is_path = inner.as_deref().is_some_and(|s| s.starts_with("$."));
                let reason = if wrapped && is_path { "path inside a literal object" } else { "not a path" };
                assert_eq!(e["reason"], json!(reason), "{ctx}");
                assert_eq!(e["path"], json!(inner.clone().unwrap()), "{ctx}");
                let actual = did_you_mean(e);
                if wrapped && is_path {
                    // A path inside a literal object: that string, unchanged.
                    assert!(actual.is_none() || actual == inner, "{ctx}");
                    refused += 1;
                    continue;
                }
                if let Some(a) = &actual {
                    // Closure: a suggestion is itself accepted.
                    assert!(fx.accepts(a, from_step).await, "did_you_mean {a} must be accepted: {ctx}");
                    assert!(a.starts_with("$."), "{ctx}");
                }
                match expected {
                    None => assert!(actual.is_none(), "no canonical spelling exists: {ctx}"),
                    // A whole-value forward reference (`$.steps[2]`) is skipped
                    // by the existing path check but is never a suggestion.
                    Some(r) if is_forward(r, from_step) => assert!(actual.is_none(), "{ctx}"),
                    Some(r) if fx.accepts(r, from_step).await => assert_eq!(actual.as_ref(), Some(r), "{ctx}"),
                    Some(r) => assert!(actual.is_none() || actual.as_ref() != Some(r), "{ctx}"),
                }
                refused += 1;
            }
            _ => panic!("unexpected outcome: {ctx}"),
        }
    }
    assert!(refused > 100, "the generator must reach the near-miss class ({refused} refused)");
}

#[tokio::test]
async fn incident_table_rows_match_their_verdict_and_canonical() {
    // The table is generated from the vendored excerpts; a stale file fails.
    let status = std::process::Command::new("python3")
        .args(["-I", "scripts/chainmap_incidents.py", "--sources", "tests/fixtures/chainmap_incident_sources/", "--check"])
        .current_dir(root_dir())
        .status()
        .expect("run scripts/chainmap_incidents.py");
    assert!(status.success(), "tests/fixtures/chainmap_incident_spellings.json is stale");

    let fx = fixture().await;
    let table = read_json("tests/fixtures/chainmap_incident_spellings.json");
    let rows = table.as_array().expect("rows");
    assert!(rows.len() >= 15, "{} rows", rows.len());
    for row in rows {
        let v = &row["spelling"];
        let from_step = row["from_step"].as_u64().expect("from_step") as usize;
        let canonical = row["canonical"].as_str();
        let outcome = fx.dry_run(v, from_step).await;
        let ctx = format!("{row} -> {outcome:?}");
        match row["expect"].as_str().expect("expect") {
            "accept" => assert!(matches!(outcome, Outcome::Accepted), "{ctx}"),
            "refuse" => {
                let Outcome::Refused(e) = &outcome else { panic!("must be refused: {ctx}") };
                let reason = row["reason"].as_str().expect("reason");
                if NEAR_MISS_REASONS.contains(&reason) || reason == "forward reference" {
                    assert_eq!(e["reason"], json!(reason), "{ctx}");
                } else {
                    // The existing path check's entries carry no `reason`.
                    assert!(e.get("reason").is_none(), "{ctx}");
                }
                let actual = did_you_mean(e);
                match canonical {
                    None => assert!(actual.is_none(), "{ctx}"),
                    Some(c) if NEAR_MISS_REASONS.contains(&reason) || reason == "missing hop" => {
                        assert_eq!(actual.as_deref(), Some(c), "{ctx}")
                    }
                    // An unknown field: the existing check's own suggestion,
                    // which names none when the fixture's fields are far.
                    Some(c) => assert!(actual.is_none() || actual.as_deref() == Some(c), "{ctx}"),
                }
            }
            other => panic!("expect {other}"),
        }
        if let Some(c) = canonical {
            assert!(fx.accepts(c, from_step).await, "canonical {c} must be accepted against the fixture: {ctx}");
        }
    }
}

#[test]
fn the_roots_the_property_covers_are_the_table() {
    let names: BTreeSet<&str> = roots().iter().map(|r| r.name).collect();
    assert_eq!(names.len(), 7, "{names:?}");
}

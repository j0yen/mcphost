//! Class-closing check for ETXTBSY ("Text file busy"): a test file that
//! writes, copies or chmods files must not `Command::new(<non-literal>)`
//! directly. A program path the test built is a file it may just have
//! written, and exec'ing it races another thread's fork (see
//! support/execw.rs); it goes through `execw::script` instead. The only
//! exceptions are the file:line entries in tests/etxtbsy-allow.txt (cargo-built
//! binaries, `env!("CARGO")`, committed repo scripts), each with a reason; an
//! entry that no longer matches its line is itself a failure.

use std::collections::BTreeMap;
use std::path::PathBuf;

const WRITE_MARKERS: [&str; 9] = [
    "fs::write(",
    "fs::copy(",
    "File::create(",
    "set_mode(",
    "set_permissions(",
    "fs::hard_link(",
    "OpenOptions",
    ".write_all(",
    "copy_dir(",
];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// True when byte offset `at` of `line` sits inside a string literal
/// (odd number of unescaped quotes before it).
fn inside_string(line: &str, at: usize) -> bool {
    let (mut quotes, mut esc) = (0, false);
    for c in line[..at].chars() {
        if esc {
            esc = false;
        } else if c == '\\' {
            esc = true;
        } else if c == '"' {
            quotes += 1;
        }
    }
    quotes % 2 == 1
}

/// `file:line` of every non-literal `Command::new(` in a file that also
/// writes, copies or chmods, minus allowlisted lines. Also pushes a message
/// for every allowlist entry for this file that no longer matches.
fn scan(file: &str, src: &str, allow: &BTreeMap<String, String>, bad: &mut Vec<String>) {
    if file == "tests/support/execw.rs" || !WRITE_MARKERS.iter().any(|m| src.contains(m)) {
        return;
    }
    for (i, line) in src.lines().enumerate() {
        let key = format!("{file}:{}", i + 1);
        let trimmed = line.trim_start();
        let call = line.find("Command::new(");
        let hit = match call {
            Some(at) if !trimmed.starts_with("//") && !inside_string(line, at) => {
                !line[at + "Command::new(".len()..].starts_with('"')
            }
            _ => false,
        };
        match (hit, allow.contains_key(&key)) {
            (true, false) => bad.push(format!("{key}: Command::new(<non-literal>) in a file that writes files; use execw::script")),
            (false, true) => bad.push(format!("{key}: stale tests/etxtbsy-allow.txt entry (line is not a non-literal Command::new)")),
            _ => {}
        }
    }
}

fn load_allow() -> BTreeMap<String, String> {
    let text = std::fs::read_to_string(root().join("tests/etxtbsy-allow.txt"))
        .expect("read tests/etxtbsy-allow.txt");
    let mut m = BTreeMap::new();
    for l in text
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
    {
        let (key, reason) = l
            .split_once(" | ")
            .unwrap_or_else(|| panic!("allowlist line needs `file:line | reason`: {l}"));
        assert!(
            !reason.trim().is_empty(),
            "allowlist entry without a reason: {l}"
        );
        assert!(
            m.insert(key.trim().to_string(), reason.to_string())
                .is_none(),
            "duplicate entry: {l}"
        );
    }
    m
}

#[test]
fn etxtbsy_ac01_every_scratch_exec_goes_through_the_helper() {
    let allow = load_allow();
    let mut files: Vec<PathBuf> = Vec::new();
    for dir in [
        "tests",
        "tests/support",
        "tests/common",
        "tests/ci_sandbox_support",
    ] {
        for e in std::fs::read_dir(root().join(dir))
            .expect("read_dir")
            .flatten()
        {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "rs") {
                files.push(p);
            }
        }
    }
    files.sort();
    let mut bad = Vec::new();
    for p in &files {
        let rel = p
            .strip_prefix(root())
            .unwrap()
            .to_string_lossy()
            .to_string();
        let src = std::fs::read_to_string(p).expect("read test source");
        scan(&rel, &src, &allow, &mut bad);
    }
    for key in allow.keys() {
        let file = key.rsplit_once(':').unwrap().0;
        assert!(
            root().join(file).exists(),
            "allowlist entry for a missing file: {key}"
        );
    }
    assert!(
        bad.is_empty(),
        "ETXTBSY class check failed:\n{}",
        bad.join("\n")
    );
}

#[test]
fn etxtbsy_ac01_red_fixture_fails_and_helper_shape_passes() {
    let none = BTreeMap::new();
    let red = concat!(
        "fn t() {\n    fs::write(p, b\"x\").unwrap();\n    Command::new(",
        "scratch.join(\"x.sh\")).output();\n}\n"
    );
    let mut bad = Vec::new();
    scan("tests/fixture.rs", red, &none, &mut bad);
    assert_eq!(bad.len(), 1, "{bad:?}");
    assert!(bad[0].starts_with("tests/fixture.rs:3:"), "{bad:?}");

    let green = "fn t() {\n    fs::write(p, b\"x\").unwrap();\n    execw::script(&scratch.join(\"x.sh\")).output();\n    let _ = Command::new(\"git\");\n}\n";
    let mut bad = Vec::new();
    scan("tests/fixture.rs", green, &none, &mut bad);
    assert!(bad.is_empty(), "{bad:?}");

    // An allowlisted line passes; the same entry goes stale once the line changes.
    let mut allow = BTreeMap::new();
    allow.insert("tests/fixture.rs:3".to_string(), "reason".to_string());
    let mut bad = Vec::new();
    scan("tests/fixture.rs", red, &allow, &mut bad);
    assert!(bad.is_empty(), "{bad:?}");
    scan("tests/fixture.rs", green, &allow, &mut bad);
    assert_eq!(bad.len(), 1, "{bad:?}");
    assert!(bad[0].contains("stale"), "{bad:?}");
}

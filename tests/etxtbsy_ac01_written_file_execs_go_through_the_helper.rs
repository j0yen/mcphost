//! Class-closing check for ETXTBSY ("Text file busy"): a test file that
//! writes, copies or chmods files must not `Command::new(<non-literal>)`
//! directly. A program path the test built is a file it may just have
//! written, and exec'ing it races another thread's fork (see
//! support/execw.rs); it goes through `execw::script` instead. The only
//! exceptions are the entries in tests/etxtbsy-allow.txt (cargo-built
//! binaries, `env!("CARGO")`, committed repo scripts), each with a reason.
//! An entry is `file | <exact source line, trimmed> | reason` and admits one
//! occurrence of that exact line in that file wherever it sits, so a merge
//! that only moves the line leaves the list alone; the same line twice needs
//! two entries. Any other non-literal exec fails at its file:line, and an
//! entry with no line left to match is itself a failure.

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

/// (file, exact trimmed source line) -> how many occurrences are allowed.
type Allow = BTreeMap<(String, String), usize>;

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

/// `(line number, trimmed line)` of every non-literal `Command::new(` in a
/// file that also writes, copies or chmods; empty for any other file.
fn execs(file: &str, src: &str) -> Vec<(usize, String)> {
    if file == "tests/support/execw.rs" || !WRITE_MARKERS.iter().any(|m| src.contains(m)) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (i, line) in src.lines().enumerate() {
        let trimmed = line.trim_start();
        let hit = match line.find("Command::new(") {
            Some(at) if !trimmed.starts_with("//") && !inside_string(line, at) => {
                !line[at + "Command::new(".len()..].starts_with('"')
            }
            _ => false,
        };
        if hit {
            out.push((i + 1, line.trim().to_string()));
        }
    }
    out
}

/// Checks one file against its allowlist entries: a line found more often
/// than it is listed fails at every file:line it occurs on; an entry listed
/// more often than its line is found is stale.
fn scan(file: &str, src: &str, allow: &Allow, bad: &mut Vec<String>) {
    let mut found: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (n, text) in execs(file, src) {
        found.entry(text).or_default().push(n);
    }
    for (text, lines) in &found {
        let listed = allow
            .get(&(file.to_string(), text.clone()))
            .copied()
            .unwrap_or(0);
        if lines.len() > listed {
            let count = match listed {
                0 => String::new(),
                _ => format!(" ({} of this line, {listed} listed)", lines.len()),
            };
            for n in lines {
                bad.push(format!("{file}:{n}: Command::new(<non-literal>) in a file that writes files; use execw::script{count}"));
            }
        }
    }
    for ((f, text), &listed) in allow.iter().filter(|((f, _), _)| f == file) {
        let n = found.get(text).map_or(0, Vec::len);
        if n < listed {
            bad.push(format!(
                "{f} | {text}: stale tests/etxtbsy-allow.txt entry ({listed} listed, {n} found)"
            ));
        }
    }
}

fn parse_allow(text: &str) -> Result<Allow, String> {
    let mut m = Allow::new();
    for l in text
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
    {
        let shape = || format!("allowlist line needs `file | <exact source line> | reason`: {l}");
        let (file, rest) = l.split_once(" | ").ok_or_else(shape)?;
        let (code, reason) = rest.rsplit_once(" | ").ok_or_else(shape)?;
        if reason.trim().is_empty() {
            return Err(format!("allowlist entry without a reason: {l}"));
        }
        if !code.contains("Command::new(") {
            return Err(format!(
                "allowlist entry does not quote a `Command::new(` line: {l}"
            ));
        }
        *m.entry((file.trim().to_string(), code.trim().to_string()))
            .or_default() += 1;
    }
    Ok(m)
}

fn load_allow() -> Allow {
    let text = std::fs::read_to_string(root().join("tests/etxtbsy-allow.txt"))
        .expect("read tests/etxtbsy-allow.txt");
    parse_allow(&text).unwrap_or_else(|e| panic!("{e}"))
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
    for (file, _) in allow.keys() {
        assert!(
            root().join(file).exists(),
            "allowlist entry for a missing file: {file}"
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
    let none = Allow::new();
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
    let mut allow = Allow::new();
    allow.insert(
        (
            "tests/fixture.rs".to_string(),
            "Command::new(scratch.join(\"x.sh\")).output();".to_string(),
        ),
        1,
    );
    let mut bad = Vec::new();
    scan("tests/fixture.rs", red, &allow, &mut bad);
    assert!(bad.is_empty(), "{bad:?}");
    scan("tests/fixture.rs", green, &allow, &mut bad);
    assert_eq!(bad.len(), 1, "{bad:?}");
    assert!(bad[0].contains("stale"), "{bad:?}");
}

/// The entry follows its exact line, not the line number: moving it passes,
/// while a second copy, a changed line, another file, a file that stopped
/// writing, and an old `file:line` entry all fail.
#[test]
fn etxtbsy_ac01_entries_bind_the_exact_line_not_its_number() {
    let exec = "    Command::new(scratch.join(\"x.sh\")).output();\n";
    let body = |above: &str, copies: usize| {
        format!(
            "fn t() {{\n{above}    fs::write(p, b\"x\").unwrap();\n{}}}\n",
            exec.repeat(copies)
        )
    };
    let run = |file: &str, src: &str, allow: &Allow| {
        let mut bad = Vec::new();
        scan(file, src, allow, &mut bad);
        bad
    };
    let listed = |n: usize| {
        let mut a = Allow::new();
        a.insert(("tests/fixture.rs".to_string(), exec.trim().to_string()), n);
        a
    };

    // Line shift: two lines added above the listed exec, as an unrelated main merge does.
    assert!(run("tests/fixture.rs", &body("", 1), &listed(1)).is_empty());
    let shifted = body("    let a = 1;\n    let b = 2;\n", 1);
    assert!(run("tests/fixture.rs", &shifted, &listed(1)).is_empty());

    // Duplicate: a second copy of a listed line fails at both lines until it is listed twice.
    let bad = run("tests/fixture.rs", &body("", 2), &listed(1));
    assert_eq!(bad.len(), 2, "{bad:?}");
    assert!(
        bad[0].starts_with("tests/fixture.rs:3:") && bad[1].starts_with("tests/fixture.rs:4:"),
        "{bad:?}"
    );
    assert!(bad[0].contains("2 of this line, 1 listed"), "{bad:?}");
    assert!(run("tests/fixture.rs", &body("", 2), &listed(2)).is_empty());
    let bad = run("tests/fixture.rs", &body("", 1), &listed(2));
    assert_eq!(bad.len(), 1, "{bad:?}");
    assert!(
        bad[0].contains("stale") && bad[0].contains("2 listed, 1 found"),
        "{bad:?}"
    );

    // Changed line: a different program path is not covered, and its old entry is stale.
    let changed = body("", 1).replace("x.sh", "y.sh");
    let bad = run("tests/fixture.rs", &changed, &listed(1));
    assert_eq!(bad.len(), 2, "{bad:?}");
    assert!(
        bad.iter().any(|b| b.starts_with("tests/fixture.rs:3:")),
        "{bad:?}"
    );
    assert!(bad.iter().any(|b| b.contains("stale")), "{bad:?}");

    // New file: the same line in another file is not covered by this file's entry.
    let bad = run("tests/other.rs", &body("", 1), &listed(1));
    assert_eq!(bad.len(), 1, "{bad:?}");
    assert!(bad[0].starts_with("tests/other.rs:3:"), "{bad:?}");

    // Writer only: without a write marker the exec is not in scope, and its entry is stale.
    let no_write = body("", 1).replace("fs::write(p, b\"x\").unwrap();", "let _ = p;");
    assert!(run("tests/fixture.rs", &no_write, &Allow::new()).is_empty());
    let bad = run("tests/fixture.rs", &no_write, &listed(1));
    assert_eq!(bad.len(), 1, "{bad:?}");
    assert!(bad[0].contains("stale"), "{bad:?}");

    // Literal and commented-out execs are never findings and cannot be listed.
    let literal = "fn t() {\n    fs::write(p, b\"x\").unwrap();\n    let _ = Command::new(\"git\");\n    // Command::new(scratch.join(\"x.sh\"));\n}\n";
    assert!(run("tests/fixture.rs", literal, &Allow::new()).is_empty());

    // Entry shape: the old `file:line | reason` form, a line that is not a
    // `Command::new(` call, and a missing reason are refused; counts add up.
    assert!(parse_allow("tests/fixture.rs:3 | cargo-built mcphost binary under test\n").is_err());
    assert!(parse_allow("tests/fixture.rs | let a = 1; | reason\n").is_err());
    assert!(parse_allow("tests/fixture.rs | Command::new(bin) |  \n").is_err());
    let two = parse_allow("# c\n\ntests/fixture.rs | Command::new(bin) | r\ntests/fixture.rs | Command::new(bin) | r\n").unwrap();
    assert_eq!(
        two.get(&(
            "tests/fixture.rs".to_string(),
            "Command::new(bin)".to_string()
        )),
        Some(&2)
    );
}

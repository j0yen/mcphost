//! PRD-mcphost-dry-run-inside-savepoint-for-transactional-verbs R3: the
//! raw-transaction-SQL lint shared by `dryspt_ac03_*` (the scanner and its
//! planted fixture) and `dryspt_ac04_*` (the generated allow-list). A
//! string-literal-aware lexer strips comments, then three rules report
//! `file:line` + enclosing `fn` + matched token. Textual, not a parser.

#![allow(dead_code)]

use std::path::Path;

pub const KEYWORDS: [&str; 6] = ["BEGIN", "COMMIT", "ROLLBACK", "SAVEPOINT", "RELEASE", "END TRANSACTION"];
pub const CALLS: [&str; 5] =
    ["unchecked_transaction(", ".transaction(", "transaction_with_behavior(", ".savepoint(", "savepoint_with_name("];
/// Files the scan skips: the helper itself and the dry-run connection owner.
pub const EXEMPT: [&str; 2] = ["src/txn.rs", "src/dryrun.rs"];

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Finding {
    pub path: String,
    pub line: usize,
    pub func: String,
    pub token: String,
}

impl Finding {
    pub fn allow_line(&self) -> String {
        format!("{}\t{}\t{}", self.path, self.func, self.token)
    }
}

struct Lit {
    start: usize,
    text: String,
}

/// Lexes `src`: returns (code view with comments and string contents
/// blanked, string literals with their char offset and decoded-ish text).
fn lex(src: &[char]) -> (Vec<char>, Vec<Lit>) {
    let n = src.len();
    let mut code: Vec<char> = src.to_vec();
    let mut lits = Vec::new();
    let blank = |code: &mut Vec<char>, a: usize, b: usize| {
        for c in code.iter_mut().take(b).skip(a) {
            if *c != '\n' {
                *c = ' ';
            }
        }
    };
    let mut i = 0;
    while i < n {
        let c = src[i];
        if c == '/' && i + 1 < n && src[i + 1] == '/' {
            let mut j = i;
            while j < n && src[j] != '\n' {
                j += 1;
            }
            blank(&mut code, i, j);
            i = j;
        } else if c == '/' && i + 1 < n && src[i + 1] == '*' {
            let mut depth = 1;
            let mut j = i + 2;
            while j < n && depth > 0 {
                if src[j] == '/' && j + 1 < n && src[j + 1] == '*' {
                    depth += 1;
                    j += 2;
                } else if src[j] == '*' && j + 1 < n && src[j + 1] == '/' {
                    depth -= 1;
                    j += 2;
                } else {
                    j += 1;
                }
            }
            blank(&mut code, i, j);
            i = j;
        } else if let Some((open_end, hashes)) = raw_string_open(src, i) {
            // r"..." / r#"..."# / br"..."
            let mut j = open_end;
            let close_ok = |j: usize| src[j] == '"' && (0..hashes).all(|k| j + 1 + k < n && src[j + 1 + k] == '#');
            while j < n && !close_ok(j) {
                j += 1;
            }
            lits.push(Lit { start: open_end, text: src[open_end..j.min(n)].iter().collect() });
            let end = (j + 1 + hashes).min(n);
            blank(&mut code, open_end, j.min(n));
            i = end;
        } else if c == '"' {
            let mut j = i + 1;
            let mut text = String::new();
            while j < n && src[j] != '"' {
                if src[j] == '\\' && j + 1 < n {
                    text.push(src[j]);
                    text.push(src[j + 1]);
                    j += 2;
                } else {
                    text.push(src[j]);
                    j += 1;
                }
            }
            lits.push(Lit { start: i + 1, text });
            blank(&mut code, i + 1, j.min(n));
            i = (j + 1).min(n);
        } else if c == '\'' {
            if i + 1 < n && src[i + 1] == '\\' {
                let mut j = i + 2;
                while j < n && src[j] != '\'' {
                    j += 1;
                }
                i = (j + 1).min(n);
            } else if i + 2 < n && src[i + 2] == '\'' {
                i += 3;
            } else {
                i += 1; // lifetime / label
            }
        } else {
            i += 1;
        }
    }
    (code, lits)
}

/// If a raw string opens at `i` (`r"`, `r#"`, `br"`), returns (index just
/// past the opening quote, number of `#`). Not preceded by an ident char.
fn raw_string_open(src: &[char], i: usize) -> Option<(usize, usize)> {
    let n = src.len();
    if i > 0 && (src[i - 1].is_alphanumeric() || src[i - 1] == '_') {
        return None;
    }
    let mut j = i;
    if j < n && src[j] == 'b' {
        j += 1;
    }
    if j >= n || src[j] != 'r' {
        return None;
    }
    j += 1;
    let mut hashes = 0;
    while j < n && src[j] == '#' {
        hashes += 1;
        j += 1;
    }
    if j < n && src[j] == '"' {
        Some((j + 1, hashes))
    } else {
        None
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// First keyword `s` (trimmed, any case) starts with, at a word boundary.
fn leading_keyword(s: &str) -> Option<&'static str> {
    let up = s.trim().to_uppercase();
    KEYWORDS.iter().copied().find(|k| {
        up.starts_with(k) && up[k.len()..].chars().next().map_or(true, |c| !is_word(c))
    })
}

/// First keyword appearing anywhere in `s` (any case) at word boundaries.
fn contained_keyword(s: &str) -> Option<&'static str> {
    let up: Vec<char> = s.to_uppercase().chars().collect();
    for k in KEYWORDS {
        let kc: Vec<char> = k.chars().collect();
        if up.len() < kc.len() {
            continue;
        }
        for a in 0..=(up.len() - kc.len()) {
            if up[a..a + kc.len()] == kc[..]
                && (a == 0 || !is_word(up[a - 1]))
                && (a + kc.len() == up.len() || !is_word(up[a + kc.len()]))
            {
                return Some(k);
            }
        }
    }
    None
}

/// Per-char-offset enclosing fn names, from brace tracking over the code view.
fn enclosing_fns(code: &[char]) -> Vec<Option<String>> {
    let n = code.len();
    let mut out = vec![None; n];
    let mut stack: Vec<(String, usize)> = Vec::new();
    let mut pending: Option<String> = None;
    let mut depth = 0usize;
    let mut i = 0;
    while i < n {
        let c = code[i];
        if c == 'f'
            && i + 3 < n
            && code[i + 1] == 'n'
            && code[i + 2].is_whitespace()
            && (i == 0 || !is_word(code[i - 1]))
        {
            let mut j = i + 2;
            while j < n && code[j].is_whitespace() {
                j += 1;
            }
            let s = j;
            while j < n && is_word(code[j]) {
                j += 1;
            }
            if j > s {
                pending = Some(code[s..j].iter().collect());
            }
            for k in i..j {
                out[k] = stack.last().map(|f| f.0.clone());
            }
            i = j;
            continue;
        }
        match c {
            '{' => {
                depth += 1;
                if let Some(name) = pending.take() {
                    stack.push((name, depth));
                }
            }
            '}' => {
                if stack.last().map_or(false, |f| f.1 == depth) {
                    out[i] = stack.last().map(|f| f.0.clone());
                    stack.pop();
                    depth = depth.saturating_sub(1);
                    i += 1;
                    continue;
                }
                depth = depth.saturating_sub(1);
            }
            ';' => pending = None,
            _ => {}
        }
        out[i] = stack.last().map(|f| f.0.clone());
        i += 1;
    }
    out
}

/// Scan one file's text. `path` is only a label.
pub fn scan_text(path: &str, text: &str) -> Vec<Finding> {
    let src: Vec<char> = text.chars().collect();
    let (code, lits) = lex(&src);
    let fns = enclosing_fns(&code);
    let line_of = |off: usize| src[..off.min(src.len())].iter().filter(|c| **c == '\n').count() + 1;
    let func_at = |off: usize| fns.get(off).cloned().flatten().unwrap_or_else(|| "<module>".to_string());
    let mut out = Vec::new();

    // execute_batch( argument spans in the code view (strings blanked, so
    // parens inside literals do not confuse the matcher).
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let pat: Vec<char> = "execute_batch(".chars().collect();
    let mut i = 0;
    while i + pat.len() <= code.len() {
        if code[i..i + pat.len()] == pat[..] {
            let mut d = 1;
            let mut j = i + pat.len();
            while j < code.len() && d > 0 {
                match code[j] {
                    '(' => d += 1,
                    ')' => d -= 1,
                    _ => {}
                }
                j += 1;
            }
            spans.push((i + pat.len(), j));
            i = j;
        } else {
            i += 1;
        }
    }

    for l in &lits {
        let in_batch = spans.iter().any(|(a, b)| l.start >= *a && l.start < *b);
        let token = if let Some(k) = leading_keyword(&l.text) {
            Some(k.to_string())
        } else if in_batch {
            contained_keyword(&l.text).map(|k| format!("execute_batch({k})"))
        } else {
            None
        };
        if let Some(token) = token {
            out.push(Finding { path: path.to_string(), line: line_of(l.start), func: func_at(l.start), token });
        }
    }

    for call in CALLS {
        let cc: Vec<char> = call.chars().collect();
        let mut i = 0;
        while i + cc.len() <= code.len() {
            if code[i..i + cc.len()] == cc[..] {
                out.push(Finding {
                    path: path.to_string(),
                    line: line_of(i),
                    func: func_at(i),
                    token: call.to_string(),
                });
                i += cc.len();
            } else {
                i += 1;
            }
        }
    }
    out.sort();
    out
}

fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let mut ents: Vec<_> = std::fs::read_dir(dir).expect("read_dir").map(|e| e.unwrap().path()).collect();
    ents.sort();
    for p in ents {
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().map_or(false, |e| e == "rs") {
            out.push(p);
        }
    }
}

/// Scan every `.rs` under `<root>/src` except [`EXEMPT`].
pub fn scan_src(root: &Path) -> Vec<Finding> {
    let mut files = Vec::new();
    walk(&root.join("src"), &mut files);
    let mut out = Vec::new();
    for f in files {
        let rel = f.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
        if EXEMPT.contains(&rel.as_str()) {
            continue;
        }
        let text = std::fs::read_to_string(&f).expect("read src file");
        out.extend(scan_text(&rel, &text));
    }
    out.sort();
    out
}

/// The exact text `TXN_ALLOWLIST_WRITE=1` writes: one sorted line per finding.
pub fn render_allowlist(findings: &[Finding]) -> String {
    let mut lines: Vec<String> = findings.iter().map(Finding::allow_line).collect();
    lines.sort();
    let mut s = lines.join("\n");
    if !s.is_empty() {
        s.push('\n');
    }
    s
}

/// `Err` when the list would be longer than `ceiling` (write refused).
pub fn render_checked(findings: &[Finding], ceiling: usize) -> Result<String, String> {
    if findings.len() > ceiling {
        return Err(format!(
            "refusing to write an allow-list of {} lines: TXN_ALLOWLIST_CEILING is {ceiling}; migrate the new site to src/txn.rs::with_tx instead",
            findings.len()
        ));
    }
    Ok(render_allowlist(findings))
}

/// Multiset subset check: every finding must be covered by a list line.
/// The failure names `file:line` and points at the helper.
pub fn check_subset(findings: &[Finding], allowlist: &str) -> Result<(), String> {
    let mut budget: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for l in allowlist.lines().filter(|l| !l.trim().is_empty()) {
        *budget.entry(l).or_insert(0) += 1;
    }
    let mut bad = Vec::new();
    for f in findings {
        let key = f.allow_line();
        match budget.get_mut(key.as_str()) {
            Some(n) if *n > 0 => *n -= 1,
            _ => bad.push(format!("{}:{} (fn {}, {})", f.path, f.line, f.func, f.token)),
        }
    }
    if bad.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "raw transaction SQL outside tests/txn_allowlist.txt -- use src/txn.rs::with_tx:\n  {}",
            bad.join("\n  ")
        ))
    }
}

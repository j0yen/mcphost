//! PRD-mcphost-emitted-doc-pointers-resolve R1: the one markdown heading
//! scanner. `build.rs` pulls this file in with `#[path]` to write
//! `DocAnchor`, and the crate compiles it as `doc_scan`, so the slug rule
//! and the section cut exist exactly once. Pure `std`: `build.rs` cannot
//! link the crate it builds.

/// Longest `text` a section carries (R1: "<= 4 KB").
pub const SECTION_MAX_BYTES: usize = 4096;

/// One markdown heading and the body under it.
#[derive(Debug, Clone, PartialEq)]
pub struct Heading {
    pub level: usize,
    pub title: String,
    /// Unique within the file: a repeated slug gets `-2`, `-3`, ...
    pub slug: String,
    /// 1-based line of the heading.
    pub line: usize,
    /// Body up to the next heading of the same or higher level, sub-headings
    /// included, trimmed and cut to [`SECTION_MAX_BYTES`].
    pub text: String,
    /// First fenced block in `text`: `(lang, code)`.
    pub example: Option<(String, String)>,
}

/// `lowercase, non-alphanumerics to '-'` (runs collapsed, edges trimmed):
/// the one slugger every anchor in the crate uses.
pub fn slugify(heading: &str) -> String {
    let mut out = String::new();
    for c in heading.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

fn fence_open(line: &str) -> Option<String> {
    let t = line.trim_start();
    t.strip_prefix("```").map(|rest| rest.split_whitespace().next().unwrap_or("").to_string())
}

/// `(level, title)` of an ATX heading line.
fn heading_of(line: &str) -> Option<(usize, String)> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
        Some((hashes, line[hashes..].trim().trim_matches('#').trim().to_string()))
    } else {
        None
    }
}

fn cut(mut s: String, max: usize) -> String {
    if s.len() > max {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
    s
}

/// Every heading of `text` outside a fenced block, in file order.
pub fn scan(text: &str) -> Vec<Heading> {
    let lines: Vec<&str> = text.lines().collect();
    // (line index, level, title) of every real heading.
    let mut found: Vec<(usize, usize, String)> = Vec::new();
    let mut in_fence = false;
    for (i, line) in lines.iter().enumerate() {
        if in_fence {
            if line.trim() == "```" {
                in_fence = false;
            }
        } else if fence_open(line).is_some() {
            in_fence = true;
        } else if let Some((level, title)) = heading_of(line) {
            found.push((i, level, title));
        }
    }
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut out = Vec::new();
    for (n, (i, level, title)) in found.iter().enumerate() {
        let mut slug = slugify(title);
        if slug.is_empty() {
            slug = "section".into();
        }
        let count = seen.entry(slug.clone()).or_insert(0);
        *count += 1;
        if *count > 1 {
            slug = format!("{slug}-{count}");
        }
        let end = found[n + 1..].iter().find(|(_, l, _)| l <= level).map_or(lines.len(), |(j, _, _)| *j);
        let body = &lines[i + 1..end];
        out.push(Heading {
            level: *level,
            title: title.clone(),
            slug,
            line: i + 1,
            text: cut(body.join("\n").trim().to_string(), SECTION_MAX_BYTES),
            example: first_fence(body),
        });
    }
    out
}

/// First fenced block of `body` (an unclosed fence runs to the end).
fn first_fence(body: &[&str]) -> Option<(String, String)> {
    let start = body.iter().position(|l| fence_open(l).is_some())?;
    let lang = fence_open(body[start]).unwrap_or_default();
    let rest = &body[start + 1..];
    let close = rest.iter().position(|l| l.trim() == "```").unwrap_or(rest.len());
    Some((lang, rest[..close].join("\n")))
}

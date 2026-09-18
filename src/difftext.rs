//! A minimal unified-diff text generator for `host.tool_diff`
//! (PRD-mcphost-tool-versions P2 requirement 8 / AC9). This workspace has
//! no diff crate, and a tool spec is small (a few KB at most), so a plain
//! O(n*m) LCS line diff is plenty -- this is not meant to compete with a
//! real Myers-diff implementation on large files.

/// Longest-common-subsequence line diff between `old` and `new`, rendered
/// as a single-hunk unified diff (`--- <old_label>` / `+++ <new_label>` /
/// one `@@` header spanning the whole text / ` `, `-`, `+` lines). Good
/// enough for `host.tool_diff`'s "return a unified diff" contract (AC9)
/// without a new dependency.
pub fn unified_diff(old_label: &str, new_label: &str, old: &str, new: &str) -> String {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let n = old_lines.len();
    let m = new_lines.len();

    // lcs[i][j] = length of the LCS of old_lines[i..] and new_lines[j..].
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if old_lines[i] == new_lines[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }

    let mut body = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if old_lines[i] == new_lines[j] {
            body.push(format!(" {}", old_lines[i]));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            body.push(format!("-{}", old_lines[i]));
            i += 1;
        } else {
            body.push(format!("+{}", new_lines[j]));
            j += 1;
        }
    }
    while i < n {
        body.push(format!("-{}", old_lines[i]));
        i += 1;
    }
    while j < m {
        body.push(format!("+{}", new_lines[j]));
        j += 1;
    }

    let mut out = format!("--- {old_label}\n+++ {new_label}\n@@ -1,{n} +1,{m} @@\n");
    out.push_str(&body.join("\n"));
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_marks_added_and_removed_lines() {
        let out = unified_diff("v1", "v2", "a\nb\nc", "a\nx\nc");
        assert!(out.contains("--- v1"));
        assert!(out.contains("+++ v2"));
        assert!(out.contains("-b"));
        assert!(out.contains("+x"));
        assert!(out.contains(" a"));
        assert!(out.contains(" c"));
    }

    #[test]
    fn diff_of_identical_text_has_no_change_lines() {
        let out = unified_diff("v1", "v2", "same\ntext", "same\ntext");
        for line in out.lines().skip(3) {
            assert!(
                !line.starts_with('-') && !line.starts_with('+'),
                "unexpected change line in identical-text diff: {line}"
            );
        }
    }
}

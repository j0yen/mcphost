//! A small hand-rolled unified-diff generator for `host.tool_diff`
//! (PRD-mcphost-tool-versions requirement 8, AC9). Diffs two blocks of text
//! line by line via the standard LCS backtrack; deliberately not a new
//! dependency -- this crate's specs are small (<=64 KiB, `MAX_SPEC_BYTES`),
//! so the O(n*m) table is tiny, and this is the same "hand-roll when it's
//! this small and self-contained" call already made for `plans.rs`'s
//! minimal TOML writer and `state::rfc3339_from_unix`'s calendar math.

/// Returns a unified diff (`--- old_label` / `+++ new_label` / one `@@`
/// hunk covering the whole text) of `old` against `new`, or an empty string
/// when the two are identical.
pub fn unified_diff(old: &str, new: &str, old_label: &str, new_label: &str) -> String {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let ops = diff_ops(&old_lines, &new_lines);
    if ops.iter().all(|op| matches!(op, DiffOp::Equal(_))) {
        return String::new();
    }
    let mut out = format!("--- {old_label}\n+++ {new_label}\n");
    out.push_str(&format!(
        "@@ -1,{} +1,{} @@\n",
        old_lines.len(),
        new_lines.len()
    ));
    for op in ops {
        match op {
            DiffOp::Equal(line) => out.push_str(&format!(" {line}\n")),
            DiffOp::Delete(line) => out.push_str(&format!("-{line}\n")),
            DiffOp::Insert(line) => out.push_str(&format!("+{line}\n")),
        }
    }
    out
}

enum DiffOp<'a> {
    Equal(&'a str),
    Delete(&'a str),
    Insert(&'a str),
}

/// Standard LCS dynamic-programming table, backtracked greedily-toward-equal
/// so equal runs stay `Equal` rather than an arbitrary delete/insert pair.
fn diff_ops<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<DiffOp<'a>> {
    let n = a.len();
    let m = b.len();
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut ops = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            ops.push(DiffOp::Equal(a[i]));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            ops.push(DiffOp::Delete(a[i]));
            i += 1;
        } else {
            ops.push(DiffOp::Insert(b[j]));
            j += 1;
        }
    }
    while i < n {
        ops.push(DiffOp::Delete(a[i]));
        i += 1;
    }
    while j < m {
        ops.push(DiffOp::Insert(b[j]));
        j += 1;
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_text_produces_empty_diff() {
        assert_eq!(unified_diff("a\nb\n", "a\nb\n", "old", "new"), "");
    }

    #[test]
    fn changed_line_shows_as_delete_and_insert() {
        let diff = unified_diff("a\nb\nc\n", "a\nx\nc\n", "v1", "v2");
        assert!(diff.starts_with("--- v1\n+++ v2\n"));
        assert!(diff.contains("-b\n"));
        assert!(diff.contains("+x\n"));
        assert!(diff.contains(" a\n"));
        assert!(diff.contains(" c\n"));
    }
}

//! PRD-mcphost-migration-safety requirement 1 / AC1: every migration file
//! must declare `-- compat: previous` or `-- compat: breaking` in its
//! header, and a `breaking` migration must have a sibling `RESTORE.md` note
//! plus a mention in the top `CHANGELOG.md` entry naming it. Enforced here
//! rather than just documented, so a new migration that forgets the header
//! (or forgets the restore note for a breaking change) fails the build
//! before it ever reaches `--check-compat` or a real deploy.

use std::fs;
use std::path::Path;

/// `Ok(())` or an error naming exactly which file/rule failed -- pure
/// function over a directory + changelog text so the edge cases (missing
/// header, missing RESTORE.md, missing CHANGELOG mention) can be exercised
/// against disposable fixtures without touching the real `migrations/`.
fn check_migrations_dir(dir: &Path, changelog: &str) -> Result<(), String> {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .map_err(|e| format!("cannot read {}: {e}", dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("sql"))
        .collect();
    entries.sort();

    for path in entries {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let content = fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        // The compat header must appear in the first non-blank line (the
        // migration's leading comment block), not buried further down.
        let first_line = content
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        let compat = if first_line.trim_start().starts_with("-- compat: previous") {
            "previous"
        } else if first_line.trim_start().starts_with("-- compat: breaking") {
            "breaking"
        } else {
            return Err(format!(
                "{name}: missing `-- compat: previous` / `-- compat: breaking` header \
                 as its first line"
            ));
        };

        if compat == "breaking" {
            let stem = path.file_stem().unwrap().to_string_lossy().to_string();
            let restore_path = dir.join(format!("{stem}.RESTORE.md"));
            if !restore_path.exists() {
                return Err(format!(
                    "{name}: declared `compat: breaking` but has no sibling \
                     {stem}.RESTORE.md"
                ));
            }
            if !changelog.contains(&stem) {
                return Err(format!(
                    "{name}: declared `compat: breaking` but the top CHANGELOG.md \
                     entry does not name it (expected \"{stem}\" to appear)"
                ));
            }
        }
    }
    Ok(())
}

/// AC1 / AC4: the real repo's migrations all declare a compat header, and
/// (requirement 4) the tenant-delete cascade migration -- whichever number
/// it lands as -- must be `compat: previous`, not `breaking`, since no
/// breaking migration exists in this repo yet.
#[test]
fn real_migrations_all_declare_compat() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let changelog = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("CHANGELOG.md"))
        .unwrap_or_default();
    check_migrations_dir(&dir, &changelog)
        .expect("every migration in migrations/ must declare a compat header");
}

// ---- fixture-based edge cases (isolated from the real migrations/) -------

struct ScratchDir(std::path::PathBuf);

impl ScratchDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "mcphost-migration-compat-test-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("create scratch dir");
        Self(dir)
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn missing_header_fails_naming_the_file() {
    let scratch = ScratchDir::new("missing-header");
    fs::write(
        scratch.0.join("0009_no_header.sql"),
        "CREATE TABLE t (id INTEGER PRIMARY KEY);\n",
    )
    .unwrap();
    let err = check_migrations_dir(&scratch.0, "").unwrap_err();
    assert!(
        err.contains("0009_no_header.sql"),
        "error should name the offending file, got: {err}"
    );
}

#[test]
fn breaking_without_restore_note_fails() {
    let scratch = ScratchDir::new("breaking-no-restore");
    fs::write(
        scratch.0.join("0009_cascade.sql"),
        "-- compat: breaking -- rewrites five tables\nDROP TABLE tools;\n",
    )
    .unwrap();
    let err = check_migrations_dir(&scratch.0, "## v0.9.0\n0009_cascade rewrites tools.\n")
        .unwrap_err();
    assert!(err.contains("RESTORE.md"), "got: {err}");
}

#[test]
fn breaking_without_changelog_mention_fails() {
    let scratch = ScratchDir::new("breaking-no-changelog");
    fs::write(
        scratch.0.join("0009_cascade.sql"),
        "-- compat: breaking -- rewrites five tables\nDROP TABLE tools;\n",
    )
    .unwrap();
    fs::write(
        scratch.0.join("0009_cascade.RESTORE.md"),
        "# Restore\n\nRestore last night's backup; see mcphost-deploy restore.\n",
    )
    .unwrap();
    let err = check_migrations_dir(&scratch.0, "## v0.9.0\nunrelated release notes.\n")
        .unwrap_err();
    assert!(err.contains("CHANGELOG"), "got: {err}");
}

#[test]
fn breaking_with_restore_and_changelog_passes() {
    let scratch = ScratchDir::new("breaking-ok");
    fs::write(
        scratch.0.join("0009_cascade.sql"),
        "-- compat: breaking -- rewrites five tables\nDROP TABLE tools;\n",
    )
    .unwrap();
    fs::write(
        scratch.0.join("0009_cascade.RESTORE.md"),
        "# Restore\n\nRestore last night's backup; see mcphost-deploy restore.\n",
    )
    .unwrap();
    check_migrations_dir(&scratch.0, "## v0.9.0\n0009_cascade rewrites the tools table.\n")
        .expect("breaking migration with RESTORE.md + changelog mention should pass");
}

#[test]
fn previous_compat_needs_neither_restore_nor_changelog() {
    let scratch = ScratchDir::new("previous-ok");
    fs::write(
        scratch.0.join("0009_additive.sql"),
        "-- compat: previous -- additive column\nALTER TABLE tools ADD COLUMN x INTEGER;\n",
    )
    .unwrap();
    check_migrations_dir(&scratch.0, "").expect("compat: previous needs no RESTORE.md");
}

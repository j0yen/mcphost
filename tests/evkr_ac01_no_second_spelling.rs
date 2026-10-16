//! PRD-mcphost-event-kinds-registry-generated AC1: no second hand-spelled
//! copy of an event kind survives. Every `"run.done"` hit under src/www/docs
//! outside `src/events.rs` is in a generated file that `docs/EVENTS.md` lists.

use std::path::{Path, PathBuf};

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if matches!(p.extension().and_then(|x| x.to_str()), Some("rs" | "txt" | "md" | "json")) {
            out.push(p);
        }
    }
}

#[test]
fn evkr_ac01() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let listing = std::fs::read_to_string(root.join("docs/EVENTS.md")).expect("docs/EVENTS.md");
    let mut files = Vec::new();
    for d in ["src", "www", "docs"] {
        walk(&root.join(d), &mut files);
    }
    let mut generated_hits = 0;
    for f in files {
        let rel = f.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
        if rel == "src/events.rs" {
            continue;
        }
        let text = std::fs::read_to_string(&f).unwrap_or_default();
        if !text.contains("\"run.done\"") {
            continue;
        }
        assert!(
            listing.contains(&format!("`{rel}`")),
            "{rel} spells \"run.done\" by hand: it must derive from src/events.rs and be listed in docs/EVENTS.md"
        );
        generated_hits += 1;
    }
    assert!(generated_hits >= 1, "the generated www/event-kinds.json should carry the kind");
}

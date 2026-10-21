//! PRX: `run_proof` extracts each proof into a dir only that call owns.
//! Sixteen threads prove the same example at once (released together by a
//! Barrier); each stub proof records its own cwd and checks its own
//! proof.sh is still its own after the siblings have run.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};

use mcphost::example_proofs::{RunOpts, run};

const THREADS: usize = 16;

/// A files dir holding a stub proof for every embedded example. Each stub
/// writes its cwd to `cwd_file`, waits so siblings overlap, then fails
/// unless `$0` still carries this thread's `tag`.
fn stub_for(tag: &str, cwd_file: &Path) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("exprf-ac08-stub-{}-{n}-{tag}", std::process::id()));
    for ex in mcphost::examples::EXAMPLES {
        let d = dir.join(ex.name).join(Path::new(ex.proof).parent().unwrap_or(Path::new("")));
        std::fs::create_dir_all(&d).expect("stub dir");
        let body = format!(
            "#!/usr/bin/env bash\n# {tag}\necho \"$PWD\" > '{cwd}'\nsleep 1\ngrep -q '# {tag}' \"$0\" || exit 1\necho \"own {tag}\"\nexit 0\n",
            cwd = cwd_file.display()
        );
        std::fs::write(d.join(Path::new(ex.proof).file_name().expect("proof file name")), body).expect("stub proof");
    }
    dir
}

#[test]
fn exprf_ac08() {
    let barrier = Arc::new(Barrier::new(THREADS));
    let handles: Vec<_> = (0..THREADS)
        .map(|i| {
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let tag = format!("THREAD-{i}-END");
                let cwd_file = std::env::temp_dir().join(format!("exprf-ac08-cwd-{}-{i}", std::process::id()));
                let _ = std::fs::remove_file(&cwd_file);
                let files_dir = stub_for(&tag, &cwd_file);
                let opts = RunOpts {
                    url: "http://127.0.0.1:1/mcp".to_string(),
                    only: Some("uptime-probes".to_string()),
                    report: false,
                    dry_run: false,
                    admin_key: None,
                    files_dir: Some(files_dir.clone()),
                };
                let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
                barrier.wait();
                let (runs, all_ok) = rt.block_on(run(&opts, &mut Vec::new())).expect("run");
                let cwd = std::fs::read_to_string(&cwd_file).unwrap_or_default().trim().to_string();
                let _ = std::fs::remove_file(&cwd_file);
                let _ = std::fs::remove_dir_all(&files_dir);
                (tag, runs, all_ok, cwd)
            })
        })
        .collect();

    let mut dirs = HashSet::new();
    for h in handles {
        let (tag, runs, all_ok, cwd) = h.join().expect("thread");
        assert_eq!(runs.len(), 1, "{tag}: one proof ran");
        let log = &runs[0].log_excerpt;
        assert!(all_ok && runs[0].ok, "{tag}: proof saw its own script, not a sibling's; log: {log}");
        assert!(log.contains(&format!("own {tag}")), "{tag}: sees its own marker; log: {log}");
        assert_eq!(log.matches("THREAD-").count(), 1, "{tag}: sees exactly one marker; log: {log}");
        assert!(!cwd.is_empty(), "{tag}: the stub recorded its extract dir");
        assert!(!Path::new(&cwd).exists(), "{tag}: extract dir {cwd} is gone after return");
        assert!(dirs.insert(cwd.clone()), "{tag}: extract dir {cwd} was shared with a sibling");
    }
    assert_eq!(dirs.len(), THREADS, "16 distinct extract dirs");
}

/// Drift: no `temp_dir()` path build in `src/` carries a seconds timestamp
/// (the shape that made two same-second calls share one dir).
#[test]
fn exprf_ac08_no_second_stamped_temp_dir_in_src() {
    fn walk(dir: &Path, hits: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).expect("read src").flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, hits);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                let mut from = 0;
                while let Some(off) = text[from..].find("temp_dir()") {
                    let start = from + off;
                    let end = text[start..].find(';').map_or(text.len(), |e| start + e);
                    let stmt = &text[start..end];
                    if stmt.contains("as_secs()") || stmt.contains("now_unix") {
                        hits.push(format!("{}: {}", path.display(), stmt.replace('\n', " ")));
                    }
                    from = start + "temp_dir()".len();
                }
            }
        }
    }
    let mut hits = Vec::new();
    walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut hits);
    assert!(hits.is_empty(), "second-stamped temp_dir() path builds in src/: {hits:#?}");
}

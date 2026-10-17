//! PRD-mcphost-launch-check-generated: the launch checklist as data.
//! `launch/items.toml` holds one `[[item]]` per launch claim; `mcphost
//! launch-check` runs the `host` probes and `--render` writes
//! `docs/launch/wave1-checklist.md` from the same rows.

use std::path::Path;

use serde::Deserialize;

pub const ITEMS_PATH: &str = "launch/items.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Owner {
    Host,
    Joe,
}

impl Owner {
    pub fn as_str(self) -> &'static str {
        match self {
            Owner::Host => "host",
            Owner::Joe => "joe",
        }
    }
}

fn default_timeout_s() -> u64 {
    15
}

#[derive(Debug, Clone, Deserialize)]
pub struct Item {
    pub id: String,
    pub wave: u8,
    pub owner: Owner,
    #[serde(default)]
    pub probe: Option<String>,
    /// `exit0`, or a literal substring of the probe's stdout.
    #[serde(default)]
    pub expect: String,
    #[serde(default = "default_timeout_s")]
    pub timeout_s: u64,
    #[serde(default)]
    pub doc: String,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Deserialize)]
struct ItemsFile {
    #[serde(default)]
    item: Vec<Item>,
}

/// Parse a `launch/items.toml` body.
pub fn parse_items(text: &str) -> Result<Vec<Item>, String> {
    let file: ItemsFile = toml::from_str(text).map_err(|e| e.to_string())?;
    Ok(file.item)
}

/// Read and parse an items file.
pub fn load_items_file(path: &Path) -> Result<Vec<Item>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_items(&text).map_err(|e| format!("{}: {e}", path.display()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Green,
    Red,
    Manual,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Green => "green",
            Status::Red => "red",
            Status::Manual => "manual",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ItemResult {
    pub id: String,
    pub owner: Owner,
    pub status: Status,
    /// `red`: why (≤ 120 chars); `manual`: the item's `doc`; `green`: empty.
    pub reason: String,
    pub duration_ms: u64,
}

fn clip(s: &str, max: usize) -> String {
    let one_line: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    one_line.chars().take(max).collect()
}

/// Run one `host` probe through `sh -c` with a clean environment (`PATH`
/// and `MCPHOST_PUBLIC_URL` only) and judge it against `expect`.
pub fn run_probe(item: &Item) -> ItemResult {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let start = Instant::now();
    let done = |status: Status, reason: String| ItemResult {
        id: item.id.clone(),
        owner: item.owner,
        status,
        reason,
        duration_ms: start.elapsed().as_millis() as u64,
    };
    let Some(probe) = item.probe.as_deref().filter(|p| !p.trim().is_empty()) else {
        return done(Status::Red, "host item has no probe".into());
    };
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(probe).env_clear();
    for var in ["PATH", "MCPHOST_PUBLIC_URL"] {
        if let Ok(v) = std::env::var(var) {
            cmd.env(var, v);
        }
    }
    // Own process group so a timed-out probe's children die with it.
    cmd.process_group(0).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return done(Status::Red, clip(&format!("spawn failed: {e}"), 120)),
    };
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return done(Status::Red, "probe stdout not captured".into());
    };
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    });
    let deadline = start + Duration::from_secs(item.timeout_s);
    let exit = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if Instant::now() >= deadline => {
                // SAFETY: killpg on the group this function created.
                unsafe { libc::killpg(child.id() as i32, libc::SIGKILL) };
                let _ = child.wait();
                // The reader thread may be held open by a grandchild; detach it.
                return done(Status::Red, format!("timeout {}s", item.timeout_s));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return done(Status::Red, clip(&format!("wait failed: {e}"), 120)),
        }
    };
    let out = reader.join().unwrap_or_default();
    if item.expect == "exit0" {
        if exit.success() {
            done(Status::Green, String::new())
        } else {
            let code = exit.code().map_or("signal".to_string(), |c| c.to_string());
            done(Status::Red, format!("exit {code}"))
        }
    } else if out.contains(&item.expect) {
        done(Status::Green, String::new())
    } else {
        done(Status::Red, clip(&format!("expected {:?}", item.expect), 120))
    }
}

/// Run every item (optionally only wave `wave` / id `only`): `host` items
/// are probed, `joe` items become `manual`.
pub fn run_items(items: &[Item], wave: Option<u8>, only: Option<&str>) -> Vec<ItemResult> {
    items
        .iter()
        .filter(|i| wave.is_none_or(|w| i.wave == w))
        .filter(|i| only.is_none_or(|o| i.id == o))
        .map(|i| match i.owner {
            Owner::Host => run_probe(i),
            Owner::Joe => ItemResult {
                id: i.id.clone(),
                owner: i.owner,
                status: Status::Manual,
                reason: i.doc.clone(),
                duration_ms: 0,
            },
        })
        .collect()
}

/// `(green, total)` where total counts `host` items only.
pub fn tally(results: &[ItemResult]) -> (usize, usize) {
    let green = results.iter().filter(|r| r.status == Status::Green).count();
    let total = results.iter().filter(|r| r.owner == Owner::Host).count();
    (green, total)
}

/// One printed line per result, then the `n/m green` summary.
pub fn render_text(results: &[ItemResult]) -> String {
    let mut out = String::new();
    for r in results {
        match r.status {
            Status::Green => out.push_str(&format!("green {}\n", r.id)),
            Status::Red => out.push_str(&format!("red {} ({})\n", r.id, clip(&r.reason, 120))),
            Status::Manual => out.push_str(&format!("manual {}: {}\n", r.id, r.reason)),
        }
    }
    let (green, total) = tally(results);
    out.push_str(&format!("{green}/{total} green\n"));
    out
}

/// `{items: [{id, owner, status, reason, duration_ms}], green, total, wave,
/// checked_at}` for `--json`.
pub fn render_json(results: &[ItemResult], wave: Option<u8>) -> String {
    let (green, total) = tally(results);
    let items: Vec<serde_json::Value> = results
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id,
                "owner": r.owner.as_str(),
                "status": r.status.as_str(),
                "reason": clip(&r.reason, 120),
                "duration_ms": r.duration_ms,
            })
        })
        .collect();
    let v = serde_json::json!({
        "items": items,
        "green": green,
        "total": total,
        "wave": wave,
        "checked_at": crate::state::rfc3339_now(),
    });
    format!("{v}\n")
}

pub const CHECKLIST_PATH: &str = "docs/launch/wave1-checklist.md";
pub const README_PATH: &str = "README.md";

/// The external `http(s)` URLs in a README, deduplicated, in order of first
/// appearance. Placeholders (`example.com`, `localhost`, templated or
/// angle-bracketed URLs) are not links a reader can follow, so they are
/// skipped.
pub fn readme_external_links(readme: &str) -> Vec<String> {
    let mut seen = Vec::new();
    let mut rest = readme;
    while let Some(pos) = rest.find("http") {
        let tail = &rest[pos..];
        let scheme = if tail.starts_with("https://") {
            8
        } else if tail.starts_with("http://") {
            7
        } else {
            rest = &rest[pos + 4..];
            continue;
        };
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | '>' | '"' | '`' | ']' | '|' | '<' | '\'' | '\\'))
            .unwrap_or(tail.len());
        let mut url = tail[..end].trim_end_matches(['.', ',', ';', ':', '*']).to_string();
        if let Some(i) = url.find('#') {
            url.truncate(i);
        }
        rest = &rest[pos + end.max(scheme)..];
        let host = url[scheme..].split(['/', ':']).next().unwrap_or("");
        let placeholder = host.is_empty()
            || !host.contains('.')
            || host.ends_with("example.com")
            || host == "localhost"
            || url.contains(['{', '$', '<']);
        if !placeholder && !seen.contains(&url) {
            seen.push(url);
        }
    }
    seen
}

fn host_slug(url: &str) -> String {
    let after = url.split("://").nth(1).unwrap_or(url);
    let host = after.split(['/', ':']).next().unwrap_or(after);
    host.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect()
}

/// The generated `readme-link-<host>` rows: one per external host in the
/// README, its probe fetching every README URL on that host.
pub fn readme_link_items(readme: &str) -> Vec<Item> {
    let mut by_host: Vec<(String, Vec<String>)> = Vec::new();
    for url in readme_external_links(readme) {
        let slug = host_slug(&url);
        match by_host.iter_mut().find(|(h, _)| *h == slug) {
            Some((_, urls)) => urls.push(url),
            None => by_host.push((slug, vec![url])),
        }
    }
    by_host
        .into_iter()
        .map(|(slug, urls)| Item {
            id: format!("readme-link-{slug}"),
            wave: 1,
            owner: Owner::Host,
            probe: Some(format!(
                "for u in {}; do curl -fsSL -m 10 -o /dev/null \"$u\" || exit 1; done",
                urls.iter().map(|u| format!("'{u}'")).collect::<Vec<_>>().join(" ")
            )),
            expect: "exit0".into(),
            timeout_s: 15 * urls.len() as u64,
            doc: format!("Every README link on {slug} resolves: {}.", urls.join(", ")),
            url: urls.first().cloned(),
        })
        .collect()
}

/// The TOML rows plus the README-generated link rows, sorted by (wave, id).
pub fn effective_items(mut items: Vec<Item>, readme: Option<&str>) -> Vec<Item> {
    if let Some(readme) = readme {
        items.extend(readme_link_items(readme));
    }
    items.sort_by(|a, b| (a.wave, &a.id).cmp(&(b.wave, &b.id)));
    items
}

fn cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

/// `docs/launch/wave1-checklist.md` from the rows: one table per wave,
/// deterministic (sorted, no timestamps). The last column is left blank for
/// a human or a nightly run to fill in.
pub fn render_checklist(items: &[Item]) -> String {
    let mut out = String::from(
        "<!-- Generated by `mcphost launch-check --render` from launch/items.toml and README.md. Do not edit by hand: edit the TOML and re-render. -->\n\n# Launch checklist\n\nEvery `host` row is a probe `mcphost launch-check` runs; every `joe` row is a manual step.\n",
    );
    let mut waves: Vec<u8> = items.iter().map(|i| i.wave).collect();
    waves.sort_unstable();
    waves.dedup();
    for w in waves {
        out.push_str(&format!(
            "\n## Wave {w}\n\n| id | owner | what it proves | probe or instruction | last result |\n|---|---|---|---|---|\n"
        ));
        for i in items.iter().filter(|i| i.wave == w) {
            let how = match i.owner {
                Owner::Host => format!("`{}` expects {}", cell(i.probe.as_deref().unwrap_or("")), cell(&i.expect)),
                Owner::Joe => match &i.url {
                    Some(u) => format!("{} ({})", cell(&i.doc), cell(u)),
                    None => cell(&i.doc),
                },
            };
            out.push_str(&format!(
                "| {} | {} | {} | {} |  |\n",
                cell(&i.id),
                i.owner.as_str(),
                cell(&i.doc),
                how
            ));
        }
    }
    out
}

/// Write a run's `--json` body to `<dir>/launch-check-<UTC date>.json`. A
/// file already there (an earlier run the same UTC day) is kept as
/// `launch-check-<date>.1.json`, replacing any older `.1`. Returns the path
/// written.
pub fn write_run_json(dir: &Path, body: &str) -> std::io::Result<std::path::PathBuf> {
    std::fs::create_dir_all(dir)?;
    let date = crate::state::date_from_unix(crate::state::now_unix());
    let latest = dir.join(format!("launch-check-{date}.json"));
    if latest.exists() {
        std::fs::rename(&latest, dir.join(format!("launch-check-{date}.1.json")))?;
    }
    std::fs::write(&latest, body)?;
    Ok(latest)
}

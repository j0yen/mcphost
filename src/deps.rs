//! `python`-kind dependency resolution, hash-locking, package policy and
//! advisory checking. PRD-mcphost-python-dependency-policy.
//!
//! Lives outside `kinds::python` because it needs `AppState`/`Db` (the
//! durable `tool_lock` ledger, the daily re-audit) -- machinery
//! `kinds::python::PythonKind` deliberately doesn't have wired to it (see
//! that module's own doc comment on why: no `Kind` trait method carries
//! `AppState`). `control::tool_publish` is the only caller that resolves a
//! lock; `kinds::python` only ever *reads* one back out of a spec it
//! already has in hand (the `_dependency_lock` field `tool_publish`
//! injects before storing).

use serde::Deserialize;
use serde_json::{Value, json};

use crate::errors::AppError;
use crate::state::AppState;

/// requirement 1: at most this many bytes of `stdout`+`stderr` kept on a
/// failed `uv pip compile` -- same tail-capping convention
/// `kinds::python::run_command_tail` uses for a failed env build.
const RESOLVE_FAIL_TAIL_BYTES: usize = 4096;

/// requirement 6: the real production cadence of the unattended re-audit
/// loop.
const REAUDIT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(86_400);

// ---- package policy (requirement 3, AC5) -----------------------------

/// requirement 3: `MCPHOST_PKG_DENY` default -- a short, operator-owned
/// seed list of names known to be abused as typosquat targets or otherwise
/// unwelcome; no canonical source, an operator overrides the whole list via
/// the env var (comma-separated).
const DEFAULT_DENYLIST: &[&str] = &[
    "python-mysql",
    "setup-tools",
    "beautifulsoup",
    "urlib3",
    "django-server",
    "crypt",
];

/// requirement 3: "Levenshtein-1 from the top-100 PyPI names" -- an
/// approximate, hand-maintained list of the most-downloaded real PyPI
/// project names (not live-fetched; the PRD's own goal is a fixed,
/// predictable policy, not a moving target). Compared PEP 503-normalized.
const TOP_100_PYPI: &[&str] = &[
    "requests", "urllib3", "boto3", "botocore", "numpy", "pandas", "setuptools", "pip", "wheel",
    "certifi", "charset-normalizer", "idna", "six", "python-dateutil", "pyyaml", "cryptography",
    "click", "jinja2", "markupsafe", "packaging", "pytz", "s3transfer", "typing-extensions",
    "attrs", "importlib-metadata", "wrapt", "platformdirs", "aiobotocore", "protobuf",
    "google-api-core", "googleapis-common-protos", "rsa", "pyasn1", "oauthlib",
    "requests-oauthlib", "pynacl", "cffi", "pycparser", "colorama", "tqdm", "filelock", "fsspec",
    "huggingface-hub", "pyjwt", "grpcio", "sqlalchemy", "greenlet", "psycopg2", "pymysql", "redis",
    "celery", "kombu", "billiard", "vine", "flask", "werkzeug", "itsdangerous", "gunicorn",
    "uvicorn", "fastapi", "pydantic", "pydantic-core", "starlette", "anyio", "sniffio", "httpx",
    "httpcore", "h11", "websockets", "aiohttp", "aiosignal", "multidict", "yarl", "frozenlist",
    "async-timeout", "jsonschema", "jsonschema-specifications", "referencing", "rpds-py",
    "pyparsing", "docutils", "sphinx", "babel", "pygments", "markdown", "pyarrow", "scipy",
    "scikit-learn", "matplotlib", "pillow", "opencv-python", "torch", "tensorflow",
    "transformers", "tokenizers", "regex", "cachetools", "google-auth", "decorator",
    "more-itertools", "zipp", "tomli", "iniconfig", "pluggy", "pytest", "coverage", "mypy",
];

/// PEP 503 name normalization: runs of `-`/`_`/`.` collapse to a single
/// `-`, lowercased -- the same equivalence PyPI itself treats package names
/// under (`Foo_Bar.Baz` and `foo-bar-baz` name the same project).
fn pep503_normalize(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_was_sep = false;
    for c in name.trim().chars() {
        if c == '-' || c == '_' || c == '.' {
            if !last_was_sep && !out.is_empty() {
                out.push('-');
                last_was_sep = true;
            }
        } else {
            out.push(c.to_ascii_lowercase());
            last_was_sep = false;
        }
    }
    out.trim_end_matches('-').to_string()
}

fn denylist() -> Vec<String> {
    match std::env::var("MCPHOST_PKG_DENY") {
        Ok(v) => v
            .split(',')
            .map(pep503_normalize)
            .filter(|s| !s.is_empty())
            .collect(),
        Err(_) => DEFAULT_DENYLIST.iter().map(|s| pep503_normalize(s)).collect(),
    }
}

/// Restricted (optimal string alignment) Damerau-Levenshtein distance,
/// `<= 1`: plain Levenshtein distance would score an adjacent-character
/// transposition (`reqeusts` vs `requests`) as 2, but AC5's own example is
/// exactly that transposition -- this scores it 1, matching common
/// typosquat-detection practice (and the AC).
fn damerau_distance_le1(a: &str, b: &str) -> bool {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (la, lb) = (a.len(), b.len());
    if la.abs_diff(lb) > 1 {
        return false;
    }
    let mut d = vec![vec![0usize; lb + 1]; la + 1];
    for (i, row) in d.iter_mut().enumerate().take(la + 1) {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate().take(lb + 1) {
        *cell = j;
    }
    for i in 1..=la {
        for j in 1..=lb {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[la][lb] <= 1
}

/// requirement 3 (AC5): every package name `input` would install, checked
/// against the denylist and the top-100 near-name policy -- called after
/// [`classify`] so a caller-supplied lock's own package names are checked
/// exactly like a plain name list would be, not skipped.
pub fn check_package_policy(input: &RequirementsInput) -> Result<(), AppError> {
    let names: Vec<String> = match input {
        RequirementsInput::Names(names) => names
            .iter()
            .map(|r| crate::kinds::python::requirement_name(r).to_string())
            .collect(),
        RequirementsInput::Lock(text) => {
            parse_lock_packages(text).into_iter().map(|(n, _)| n).collect()
        }
    };
    let deny = denylist();
    let top100: Vec<String> = TOP_100_PYPI.iter().map(|s| pep503_normalize(s)).collect();
    for raw in &names {
        let normalized = pep503_normalize(raw);
        if normalized.is_empty() {
            continue;
        }
        if deny.contains(&normalized) {
            return Err(AppError::dependency_policy_denied(&format!(
                "{normalized} is denylisted"
            )));
        }
        if top100.contains(&normalized) {
            // requirement 3: "unless it is that name" -- an exact top-100
            // match is never itself flagged as a near-name of anything.
            continue;
        }
        if let Some(target) = top100.iter().find(|t| damerau_distance_le1(&normalized, t)) {
            return Err(AppError::dependency_policy_denied(&format!(
                "near-name of {target}"
            )));
        }
    }
    Ok(())
}

// ---- lock text parsing -------------------------------------------------

/// Joins a `uv pip compile --generate-hashes` (or hand-built,
/// requirement-5) requirements-file logical line, folding `\`-continued
/// lines together and dropping comments, then hands each logical line to
/// `f`. Shared by [`parse_lock_packages`] (lenient: just wants
/// `name==version`) and [`try_parse_hashed_lock`] (strict: also demands a
/// well-formed `--hash=sha256:` on every line).
fn for_each_logical_line(text: &str, mut f: impl FnMut(&str) -> bool) -> bool {
    let mut buf = String::new();
    for raw_line in text.lines() {
        let line = raw_line.trim_end();
        if buf.is_empty() && line.trim_start().starts_with('#') {
            continue;
        }
        if let Some(stripped) = line.strip_suffix('\\') {
            buf.push_str(stripped.trim_end());
            buf.push(' ');
            continue;
        }
        buf.push_str(line);
        let logical = buf.trim().to_string();
        buf.clear();
        if logical.is_empty() || logical.starts_with('#') {
            continue;
        }
        if !f(&logical) {
            return false;
        }
    }
    true
}

/// Lenient: every `name==version` logical line, hashes (or anything else
/// on the line) ignored -- used on lock text this process just generated
/// itself (trusted), or already hash-validated by [`try_parse_hashed_lock`].
pub(crate) fn parse_lock_packages(text: &str) -> Vec<(String, String)> {
    let mut packages = Vec::new();
    for_each_logical_line(text, |logical| {
        if let Some(head) = logical.split_whitespace().next()
            && let Some((name, version)) = head.split_once("==")
            && !name.is_empty()
            && !version.is_empty()
        {
            packages.push((name.to_string(), version.to_string()));
        }
        true
    });
    packages
}

/// Strict (requirement 5 / AC7): `Some(packages)` only if EVERY non-comment
/// logical line in `text` is `name==version` followed by one or more
/// well-formed `--hash=sha256:<64 lowercase hex>` tokens and nothing else
/// -- the `--require-hashes` shape `uv pip sync` itself demands. `None` on
/// the first line that doesn't fit (including "no packages at all"), so
/// [`classify`] falls back to treating the input as plain names.
fn try_parse_hashed_lock(text: &str) -> Option<Vec<(String, String)>> {
    let mut packages = Vec::new();
    let ok = for_each_logical_line(text, |logical| {
        let mut tokens = logical.split_whitespace();
        let Some(head) = tokens.next() else {
            return false;
        };
        let Some((name, version)) = head.split_once("==") else {
            return false;
        };
        if name.is_empty() || version.is_empty() {
            return false;
        }
        let mut has_hash = false;
        for tok in tokens {
            let Some(hex) = tok.strip_prefix("--hash=sha256:") else {
                return false;
            };
            if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
                return false;
            }
            has_hash = true;
        }
        if !has_hash {
            return false;
        }
        packages.push((name.to_string(), version.to_string()));
        true
    });
    if ok && !packages.is_empty() {
        Some(packages)
    } else {
        None
    }
}

// ---- resolution ---------------------------------------------------------

/// What a publish's `requirements` array turned out to be, once
/// [`classify`] has looked at it.
#[derive(Debug, Clone)]
pub enum RequirementsInput {
    /// Plain PyPI names (the common case): resolved fresh via
    /// `uv pip compile --generate-hashes`.
    Names(Vec<String>),
    /// requirement 5 (AC7): every array entry, joined by newline, already
    /// forms a complete `--require-hashes`-shaped lock -- stored as-is, no
    /// resolution performed.
    Lock(String),
}

/// requirement 5 (AC7): recognizes an agent-supplied pre-resolved lock
/// (every `requirements` entry, joined by `\n`, parses as a
/// `--require-hashes` lock) vs. an ordinary name list -- the overwhelming
/// majority of publishes, which don't remotely look like a lock line
/// (`try_parse_hashed_lock` returns `None` at the very first entry, since
/// a bare name has no `==` or `--hash=`).
pub fn classify(requirements: &[String]) -> RequirementsInput {
    let joined = requirements.join("\n");
    match try_parse_hashed_lock(&joined) {
        Some(_) => RequirementsInput::Lock(joined),
        None => RequirementsInput::Names(requirements.to_vec()),
    }
}

/// A resolved (or validated-as-supplied) dependency lock, ready to store
/// and to build an env from.
pub struct ResolvedDependencies {
    pub lock_text: String,
    pub packages: usize,
    pub advisories: Vec<AdvisoryHit>,
}

/// requirement 1/5: turns a [`RequirementsInput`] into a
/// [`ResolvedDependencies`] -- resolves fresh (network, `uv pip compile`)
/// for [`RequirementsInput::Names`], or just re-validates and re-checks
/// advisories for an already-hashed [`RequirementsInput::Lock`] (never
/// re-resolves it -- AC7's whole point).
pub async fn resolve(input: RequirementsInput) -> Result<ResolvedDependencies, AppError> {
    match input {
        RequirementsInput::Names(names) => resolve_names(&names).await,
        RequirementsInput::Lock(text) => {
            let packages = try_parse_hashed_lock(&text).ok_or_else(|| {
                AppError::dependency_resolve_failed(
                    "supplied lock failed hash validation after classification",
                )
            })?;
            let advisories = check_advisories(&packages);
            Ok(ResolvedDependencies {
                lock_text: text,
                packages: packages.len(),
                advisories,
            })
        }
    }
}

async fn resolve_names(names: &[String]) -> Result<ResolvedDependencies, AppError> {
    let scratch = std::env::temp_dir().join(format!(
        "mcphost-deps-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ));
    tokio::fs::create_dir_all(&scratch)
        .await
        .map_err(|e| AppError::Internal(format!("dependency resolve scratch dir: {e}")))?;
    let reqs_path = scratch.join("requirements.in");
    let lock_path = scratch.join("lock.txt");
    let write_result = tokio::fs::write(&reqs_path, names.join("\n")).await;

    let outcome = if let Err(e) = write_result {
        Err(AppError::Internal(format!("write requirements.in: {e}")))
    } else {
        let mut cmd = tokio::process::Command::new("uv");
        cmd.arg("pip")
            .arg("compile")
            .arg(&reqs_path)
            .arg("--generate-hashes")
            .arg("-o")
            .arg(&lock_path);
        match cmd.output().await {
            Err(e) => Err(AppError::Internal(format!("uv pip compile spawn: {e}"))),
            Ok(output) if output.status.success() => {
                tokio::fs::read_to_string(&lock_path)
                    .await
                    .map_err(|e| AppError::Internal(format!("read compiled lock: {e}")))
            }
            Ok(output) => {
                let mut tail = output.stdout;
                tail.extend_from_slice(&output.stderr);
                let start = tail.len().saturating_sub(RESOLVE_FAIL_TAIL_BYTES);
                let tail_text = String::from_utf8_lossy(&tail[start..]).into_owned();
                Err(AppError::dependency_resolve_failed(&tail_text))
            }
        }
    };
    let _ = tokio::fs::remove_dir_all(&scratch).await;

    let lock_text = outcome?;
    let packages = parse_lock_packages(&lock_text);
    let advisories = check_advisories(&packages);
    Ok(ResolvedDependencies {
        lock_text,
        packages: packages.len(),
        advisories,
    })
}

// ---- advisories (requirement 2, AC3/AC8) --------------------------------

#[derive(Debug, Clone, Deserialize)]
struct AdvisoryEntry {
    package: String,
    version: String,
    id: String,
    fixed: String,
}

/// requirement 2: a small, offline, embedded advisory table -- a real
/// vulnerability database (Non-goals: no scanning infrastructure ships
/// with this PRD) is out of scope; this is the seed "offline database"
/// requirement 2 names, refreshed in production by
/// [`spawn_reaudit_scheduler`]'s daily loop (which re-runs
/// [`check_advisories`] against whatever `$MCPHOST_ADVISORY_DB_PATH`
/// resolves to at that moment -- an operator rotates the file, not this
/// binary, to add a newly-published advisory).
fn embedded_advisories() -> Vec<AdvisoryEntry> {
    vec![AdvisoryEntry {
        package: "urllib3".to_string(),
        version: "1.26.4".to_string(),
        id: "GHSA-5phf-pp7p-vc2r".to_string(),
        fixed: "1.26.5".to_string(),
    }]
}

/// `$MCPHOST_ADVISORY_DB_PATH`, when set and parseable, replaces the
/// embedded table wholesale -- how [`spawn_reaudit_scheduler`]'s "daily"
/// refresh, and this crate's own tests (AC8: a "new" advisory appearing
/// between a publish and a re-audit), both work without needing a real
/// vulnerability feed.
fn advisory_db() -> Vec<AdvisoryEntry> {
    if let Ok(path) = std::env::var("MCPHOST_ADVISORY_DB_PATH")
        && let Ok(text) = std::fs::read_to_string(path)
        && let Ok(entries) = serde_json::from_str::<Vec<AdvisoryEntry>>(&text)
    {
        return entries;
    }
    embedded_advisories()
}

#[derive(Debug, Clone)]
pub struct AdvisoryHit {
    pub id: String,
    pub package: String,
    pub version: String,
    pub fixed: String,
}

/// requirement 2: every `(package, version)` pair with a known advisory
/// and fix -- pure and offline (no network), so both publish-time
/// (`resolve`) and the re-audit path (`reaudit_once`) can share it.
pub fn check_advisories(packages: &[(String, String)]) -> Vec<AdvisoryHit> {
    let db = advisory_db();
    let mut hits = Vec::new();
    for (name, version) in packages {
        let normalized = pep503_normalize(name);
        for entry in &db {
            if pep503_normalize(&entry.package) == normalized && entry.version == *version {
                hits.push(AdvisoryHit {
                    id: entry.id.clone(),
                    package: name.clone(),
                    version: version.clone(),
                    fixed: entry.fixed.clone(),
                });
            }
        }
    }
    hits
}

pub fn advisories_to_json(hits: &[AdvisoryHit]) -> Value {
    Value::Array(
        hits.iter()
            .map(|h| {
                json!({
                    "id": h.id,
                    "package": h.package,
                    "version": h.version,
                    "fixed": h.fixed,
                })
            })
            .collect(),
    )
}

// ---- daily re-audit (requirement 6, AC8) --------------------------------

/// requirement 6 (AC8): re-checks every currently-active tool's stored
/// lock against the (possibly refreshed) advisory database and records the
/// result on its `tool_lock` row -- never touches `tools`/`tool_versions`,
/// so a newly-flagged tool keeps running exactly as before (requirement 6:
/// "without disabling them"). Shared by the unattended daily loop
/// ([`spawn_reaudit_scheduler`]) and `admin.dependency_reaudit`'s on-demand
/// trigger.
pub async fn reaudit_once(state: &AppState) -> Result<Value, AppError> {
    let locks = state.db.list_current_tool_locks().await?;
    let now = crate::state::now_unix();
    let mut tools_flagged = 0i64;
    for (tenant_id, name, version, lock_text) in locks {
        let packages = parse_lock_packages(&lock_text);
        let hits = check_advisories(&packages);
        if !hits.is_empty() {
            tools_flagged += 1;
        }
        let advisories_json = advisories_to_json(&hits).to_string();
        state
            .db
            .update_tool_lock_advisories(tenant_id, name, version, advisories_json, now)
            .await?;
    }
    Ok(json!({"checked_unix": now, "tools_flagged": tools_flagged}))
}

/// Production entry point: called once at `mcphost serve` startup, fires
/// [`reaudit_once`] every [`REAUDIT_INTERVAL`] (24h) forever. Same
/// "unattended background loop, `admin.*` on-demand trigger shares the
/// real work" shape as `retention::spawn_prune_scheduler`.
pub fn spawn_reaudit_scheduler(state: AppState) -> tokio::task::JoinHandle<()> {
    spawn_reaudit_loop(state, None)
}

/// Test-only: same loop, firing every `interval` instead of waiting out a
/// real day -- same rationale as `retention::spawn_prune_scheduler_for_test`.
pub fn spawn_reaudit_scheduler_for_test(
    state: AppState,
    interval: std::time::Duration,
) -> tokio::task::JoinHandle<()> {
    spawn_reaudit_loop(state, Some(interval))
}

fn spawn_reaudit_loop(
    state: AppState,
    interval_override: Option<std::time::Duration>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(interval_override.unwrap_or(REAUDIT_INTERVAL)).await;
            if let Err(e) = reaudit_once(&state).await {
                tracing::warn!(error = %e, "daily dependency re-audit failed");
            }
        }
    })
}

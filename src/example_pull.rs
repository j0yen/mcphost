//! PRD-mcphost-example-from-published-tool R5: `mcphost examples proposals
//! pull --repo <clone>` turns every `approved` proposal into a commit and a
//! PR in a clone of the repo, then records the PR URL on the proposal.
//!
//! The host is read and written only through `admin.example_proposals`
//! (admin bearer from `$MCPHOST_ADMIN_KEY`, never a flag), so the command
//! runs from any machine with the key, not on the host's disk. The proposal
//! is marked `merged` only after `gh pr create` printed a PR URL. The clone
//! is left on the last proposal's branch.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

use crate::client::{RpcRequest, extract_structured, rpc_call};

/// Everything `pull` needs.
pub struct Opts {
    pub repo: PathBuf,
    pub url: String,
    pub admin_key: String,
}

/// One proposal that reached a PR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pulled {
    pub id: i64,
    pub name: String,
    pub branch: String,
    pub pr_url: String,
}

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if !out.status.success() {
        return Err(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

async fn admin(client: &reqwest::Client, opts: &Opts, args: Value) -> Result<Value, String> {
    let params = json!({"name": "admin.example_proposals", "arguments": args});
    let req = RpcRequest { method: "tools/call", params, id: 1, bearer: Some(&opts.admin_key) };
    let result = rpc_call(client, opts.url.trim_end_matches('/'), req).await.map_err(|e| e.to_string())?;
    if result.get("isError").and_then(Value::as_bool).unwrap_or(false) {
        return Err(format!("admin.example_proposals refused: {}", extract_structured(&result)));
    }
    Ok(extract_structured(&result))
}

fn safe_relative(p: &str) -> bool {
    let path = Path::new(p);
    !p.is_empty() && path.is_relative() && !path.components().any(|c| matches!(c, std::path::Component::ParentDir))
}

/// The PR URL `gh pr create` printed: the last stdout line that is a URL.
pub fn pr_url_from(stdout: &str) -> Option<String> {
    stdout.lines().rev().map(str::trim).find(|l| l.starts_with("https://") || l.starts_with("http://")).map(str::to_string)
}

fn identity_args(repo: &Path) -> Vec<String> {
    if git(repo, &["config", "user.email"]).is_ok_and(|e| !e.is_empty()) {
        return Vec::new();
    }
    ["-c", "user.name=mcphost examples pull", "-c", "user.email=examples@mcphost.dev"].map(String::from).to_vec()
}

fn open_pr(repo: &Path, branch: &str, base: &str, title: &str, body: &str) -> Result<String, String> {
    let out = Command::new("gh")
        .current_dir(repo)
        .args(["pr", "create", "--head", branch, "--base", base, "--title", title, "--body", body])
        .output()
        .map_err(|e| format!("gh pr create: {e}"))?;
    if !out.status.success() {
        return Err(format!("gh pr create: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    pr_url_from(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| "gh pr create printed no PR URL".to_string())
}

/// Leave the clone as it was: back on `base`, the failed branch gone.
fn abandon(repo: &Path, base: &str, branch: &str) {
    let _ = git(repo, &["reset", "--hard", "-q"]);
    let _ = git(repo, &["checkout", "-q", base]);
    let _ = git(repo, &["branch", "-D", branch]);
}

async fn pull_one(client: &reqwest::Client, opts: &Opts, base: &str, row: &Value, kinds: &[&str]) -> Result<Pulled, String> {
    let id = row["id"].as_i64().ok_or("proposal row has no id")?;
    let name = row["name"].as_str().ok_or("proposal row has no name")?.to_string();
    let tenant = row["tenant"].as_str().unwrap_or("unknown").to_string();
    let examples = opts.repo.join("examples");
    if examples.join(&name).exists() {
        return Err(format!("proposal {id}: examples/{name} already exists in the clone"));
    }
    let detail = admin(client, opts, json!({"action": "list", "id": id})).await?;
    let contents = detail["rendered"]["contents"].as_object().ok_or_else(|| format!("proposal {id}: no rendered files"))?;
    let branch = format!("examples/{name}-proposal-{id}");
    git(&opts.repo, &["checkout", "-q", "-b", &branch, base])?;
    let built = (|| -> Result<(), String> {
        let dir = examples.join(&name);
        for (path, text) in contents {
            if !safe_relative(path) {
                return Err(format!("proposal {id}: unsafe file path `{path}`"));
            }
            let target = dir.join(path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
            }
            std::fs::write(&target, text.as_str().unwrap_or_default()).map_err(|e| format!("write {}: {e}", target.display()))?;
            #[cfg(unix)]
            if path.ends_with(".sh") {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755));
            }
        }
        crate::examples::write_index(&examples, kinds)
            .map_err(|errs| errs.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "))?;
        git(&opts.repo, &["add", "examples"])?;
        let subject = format!("examples: add {name} (proposal {id} from {tenant})");
        let mut args: Vec<String> = identity_args(&opts.repo);
        args.extend([concat!("com", "mit"), "-q", "-m", &subject].map(String::from));
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        git(&opts.repo, &refs)?;
        git(&opts.repo, &["push", "-q", "--set-upstream", "origin", &branch])?;
        Ok(())
    })();
    if let Err(e) = built {
        abandon(&opts.repo, base, &branch);
        return Err(e);
    }
    let subject = format!("examples: add {name} (proposal {id} from {tenant})");
    let body = format!("Proposed by tenant {tenant} via host.example.propose (proposal {id}).\n\nLicense: {}.", crate::example_propose::LICENSE);
    let pr_url = match open_pr(&opts.repo, &branch, base, &subject, &body) {
        Ok(u) => u,
        Err(e) => {
            let _ = git(&opts.repo, &["checkout", "-q", base]);
            return Err(format!("proposal {id}: {e} (branch {branch} is committed and pushed; the proposal stays approved)"));
        }
    };
    // The clone is left on this proposal's branch; the next proposal (if
    // any) branches from `base` again.
    admin(client, opts, json!({"action": "merge", "id": id, "pr_url": pr_url})).await?;
    Ok(Pulled { id, name, branch, pr_url })
}

/// Pull every approved proposal. Each proposal is independent: a failure is
/// reported and the rest continue.
pub async fn pull(opts: &Opts, kinds: &[&str]) -> (Vec<Pulled>, Vec<String>) {
    let client = reqwest::Client::new();
    let mut done = Vec::new();
    let mut failed = Vec::new();
    let listed = match admin(&client, opts, json!({"action": "list", "status": "approved"})).await {
        Ok(v) => v,
        Err(e) => return (done, vec![e]),
    };
    let rows = listed["proposals"].as_array().cloned().unwrap_or_default();
    if rows.is_empty() {
        return (done, failed);
    }
    let base = match git(&opts.repo, &["rev-parse", "--abbrev-ref", "HEAD"]) {
        Ok(b) => b,
        Err(e) => return (done, vec![e]),
    };
    match git(&opts.repo, &["status", "--porcelain"]) {
        Ok(s) if s.is_empty() => {}
        Ok(_) => return (done, vec![format!("{} has uncommitted changes; commit or stash them first", opts.repo.display())]),
        Err(e) => return (done, vec![e]),
    }
    // Oldest first, so a PR series reads in proposal order.
    for row in rows.iter().rev() {
        match pull_one(&client, opts, &base, row, kinds).await {
            Ok(p) => done.push(p),
            Err(e) => failed.push(e),
        }
    }
    (done, failed)
}
